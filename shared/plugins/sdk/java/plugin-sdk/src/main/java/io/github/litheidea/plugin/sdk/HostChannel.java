package io.github.litheidea.plugin.sdk;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.RejectedExecutionException;

/**
 * The session's host-request state. The backend owns the wire; this class
 * owns what Core expects of a well-behaved client -- at most 16 unanswered
 * host requests, cancellation through {@code $/cancelRequest}, and
 * subscription events routed by ID, in order, off the reader thread.
 *
 * <p>Contract: {@code contract/v1/README.md}, "Host services".
 */
final class HostChannel {
    /** Events kept for a subscription whose creating request is still unanswered. */
    static final int MAX_EARLY_EVENTS = 128;

    /** The backend's wire. */
    interface Transport {
        /** Whether the host offered plugin functions ({@code litheFunctions}); false before {@code initialize}. */
        boolean negotiated();

        boolean ready();

        /** Writes a request; its response settles when the host answers or the session ends. */
        Sent send(String method, Object params);

        void notify(String method, Object params);

        void log(String message);

        void log(String message, Throwable error);

        /** Runs plugin code off the reader thread. */
        void execute(Runnable task);
    }

    /** A request on the wire. */
    static final class Sent {
        final String id;
        final CompletableFuture<Object> response;

        Sent(String id, CompletableFuture<Object> response) {
            this.id = id;
            this.response = response;
        }
    }

    private enum State { QUEUED, SENDING, SENT }

    /** One host request; settles for its caller once. */
    private static final class Entry {
        final String method;
        final Object params;
        final CompletableFuture<Object> future = new CompletableFuture<>();
        final List<Cancellation.Registration> registrations = new ArrayList<>();
        State state = State.QUEUED;
        boolean settled;
        /** Cancelled while being written: {@code $/cancelRequest} follows the write. */
        boolean cancelAfterWrite;
        String id;

        Entry(String method, Object params) {
            this.method = method;
            this.params = params;
        }
    }

    /** A live subscription. */
    private static final class Route {
        final String id;
        final Host.Listener listener;
        final ArrayDeque<Host.Event> pending = new ArrayDeque<>();
        volatile boolean live = true;
        long lastSequence;
        boolean draining;

        Route(String id, Host.Listener listener) {
            this.id = id;
            this.listener = listener;
        }

        boolean isLive() {
            return live;
        }
    }

    private static final class Early {
        final String subscriptionId;
        final long sequence;
        final Object event;

        Early(String subscriptionId, long sequence, Object event) {
            this.subscriptionId = subscriptionId;
            this.sequence = sequence;
            this.event = event;
        }
    }

    private final Transport transport;
    private final Object lock = new Object();
    private final ArrayDeque<Entry> queue = new ArrayDeque<>();
    private final Map<String, Route> routes = new HashMap<>();
    private final List<Early> early = new ArrayList<>();
    private int inFlight;
    /** Subscribing requests not yet answered; early events wait only while there are some. */
    private int subscribing;
    private CompletableFuture<Map<String, Object>> capabilities;
    private boolean closed;

    HostChannel(Transport transport) {
        this.transport = transport;
    }

    boolean negotiated() {
        return transport.negotiated();
    }

    /**
     * Sends {@code method} once a slot is free. Cancelling any of
     * {@code scope} rejects the caller at once with {@link CancelledException};
     * a request already sent is cancelled on the wire, and never sent again,
     * because a cancelled mutation may already have taken effect.
     */
    CompletableFuture<Object> request(String method, Object params, List<Cancellation> scope) {
        if (method == null || !method.startsWith(Protocol.HOST_PREFIX)) {
            return failed(new PluginSdkError("sdk.host.method",
                    method + " is not a host method; host requests are lithe/host/*."));
        }
        if (!open()) {
            return failed(notReady());
        }
        if (!transport.negotiated()) {
            // The answer Core would give, without the round trip.
            return failed(unavailable(method,
                    "The host did not negotiate plugin functions (litheFunctions), which offer host services."));
        }
        for (Cancellation cancellation : scope) {
            try {
                cancellation.throwIfCancelled();
            } catch (CancelledException error) {
                return failed(error);
            }
        }
        Entry entry = new Entry(method, params);
        synchronized (lock) {
            if (closed || !transport.ready()) {
                return failed(notReady());
            }
            queue.add(entry);
        }
        for (Cancellation cancellation : scope) {
            Cancellation.Registration registration = cancellation.onCancel(() -> cancel(entry, cancellation));
            boolean settled;
            synchronized (lock) {
                settled = entry.settled;
                if (!settled) {
                    entry.registrations.add(registration);
                }
            }
            if (settled) {
                registration.remove();
            }
        }
        drain();
        return entry.future;
    }

    /**
     * Sends a request whose result is {@code { subscriptionID }} and routes
     * that subscription's events to {@code listener}, one at a time and in the
     * order the host sent them.
     */
    CompletableFuture<Host.Subscription> subscribe(String method, Object params, Host.Listener listener,
            List<Cancellation> scope) {
        synchronized (lock) {
            subscribing++;
        }
        return request(method, params, scope).handle((result, error) -> {
            try {
                if (error != null) {
                    throw new CompletionException(Backend.unwrap(error));
                }
                Object id = result instanceof Map ? ((Map<?, ?>) result).get("subscriptionID") : null;
                if (!(id instanceof String) || !Protocol.isCredentialKey((String) id)) {
                    throw new PluginSdkError("sdk.host.invalidResult", method + " answered without a subscriptionID.");
                }
                String subscriptionId = (String) id;
                Route route = new Route(subscriptionId, listener);
                synchronized (lock) {
                    if (closed) {
                        throw new PluginSdkError("sdk.session.closed", "The session ended.");
                    }
                    routes.put(subscriptionId, route);
                    // Events the host sent before this answer was read.
                    for (Iterator<Early> events = early.iterator(); events.hasNext();) {
                        Early event = events.next();
                        if (event.subscriptionId.equals(subscriptionId)) {
                            events.remove();
                            emit(route, event.sequence, event.event);
                        }
                    }
                }
                return new Host.Subscription(subscriptionId, route::isLive, this);
            } finally {
                synchronized (lock) {
                    subscribing--;
                    if (subscribing == 0) {
                        early.clear();
                    }
                }
            }
        });
    }

    /** Stops routing a subscription's events at once and asks the host to dispose it. */
    CompletableFuture<Void> dispose(String subscriptionId) {
        synchronized (lock) {
            Route route = routes.remove(subscriptionId);
            if (route != null) {
                route.live = false;
                route.pending.clear();
            }
            if (closed || !transport.ready()) {
                return CompletableFuture.completedFuture(null);
            }
        }
        Map<String, Object> params = new LinkedHashMap<>();
        params.put("subscriptionID", subscriptionId);
        return request(Protocol.HOST_SUBSCRIPTIONS_DISPOSE, params, Collections.<Cancellation>emptyList())
                .thenApply(result -> null);
    }

    /** Routes one {@code lithe/host/subscriptions/notify}. */
    void deliver(Map<String, Object> params) {
        Object id = params.get("subscriptionID");
        if (!(id instanceof String)) {
            return;
        }
        Object number = params.get("sequence");
        long sequence = number instanceof Number ? ((Number) number).longValue() : 0;
        synchronized (lock) {
            Route route = routes.get(id);
            if (route != null) {
                emit(route, sequence, params.get("event"));
            } else if (subscribing > 0) {
                if (early.size() < MAX_EARLY_EVENTS) {
                    early.add(new Early((String) id, sequence, params.get("event")));
                } else {
                    transport.log("Dropped an early subscription event: queue_full");
                }
            }
            // Otherwise the subscription was disposed; nothing follows a disposal.
        }
    }

    /** The cached {@code getCapabilities} answer; Core answers it itself. */
    CompletableFuture<Map<String, Object>> capabilities() {
        if (!transport.negotiated()) {
            return failed(unavailable(Protocol.HOST_GET_CAPABILITIES,
                    "The host did not negotiate plugin functions (litheFunctions), which offer capability discovery."));
        }
        CompletableFuture<Map<String, Object>> answer;
        synchronized (lock) {
            if (capabilities != null) {
                return capabilities.thenApply(result -> result);
            }
            answer = new CompletableFuture<>();
            capabilities = answer;
        }
        request(Protocol.HOST_GET_CAPABILITIES, null, Collections.<Cancellation>emptyList()).whenComplete((result, error) -> {
            if (error == null && result instanceof Map) {
                answer.complete(Collections.unmodifiableMap(Json.object(result)));
                return;
            }
            synchronized (lock) {
                if (capabilities == answer) {
                    capabilities = null;
                }
            }
            answer.completeExceptionally(error != null ? Backend.unwrap(error)
                    : new PluginSdkError("sdk.host.invalidResult", "getCapabilities answered without an object."));
        });
        return answer.thenApply(result -> result);
    }

    /**
     * Whether this session may call {@code method}, from
     * {@code getCapabilities}; false when the host offered no plugin functions.
     * A missing grant still fails the request.
     */
    CompletableFuture<Boolean> supports(String method) {
        if (!transport.negotiated()) {
            return CompletableFuture.completedFuture(false);
        }
        return capabilities().thenApply(answer -> {
            Object services = answer.get("services");
            if (!(services instanceof Map)) {
                return false;
            }
            for (Object service : ((Map<?, ?>) services).values()) {
                Object methods = service instanceof Map ? ((Map<?, ?>) service).get("methods") : null;
                if (methods instanceof List && ((List<?>) methods).contains(method)) {
                    return true;
                }
            }
            return false;
        });
    }

    /** Ends the channel with the session: queued requests fail, subscriptions end. */
    void close() {
        List<Entry> queued;
        synchronized (lock) {
            closed = true;
            queued = new ArrayList<>(queue);
            queue.clear();
            for (Route route : routes.values()) {
                route.live = false;
                route.pending.clear();
            }
            routes.clear();
            early.clear();
            capabilities = null;
        }
        for (Entry entry : queued) {
            settle(entry, null, new PluginSdkError("sdk.session.closed", "The session ended."));
        }
    }

    private void drain() {
        List<Entry> starting = new ArrayList<>();
        synchronized (lock) {
            while (!closed && inFlight < Protocol.MAX_PENDING_HOST_REQUESTS && !queue.isEmpty()) {
                Entry entry = queue.poll();
                entry.state = State.SENDING;
                inFlight++;
                starting.add(entry);
            }
        }
        for (Entry entry : starting) {
            start(entry);
        }
    }

    /** Writes outside the lock, so a full pipe never blocks the reader thread's routing. */
    private void start(Entry entry) {
        Sent sent;
        try {
            sent = transport.send(entry.method, entry.params);
        } catch (RuntimeException error) {
            // Params JSON cannot encode never reached the wire.
            synchronized (lock) {
                inFlight--;
            }
            settle(entry, null, error);
            drain();
            return;
        }
        boolean cancelNow;
        synchronized (lock) {
            entry.id = sent.id;
            entry.state = State.SENT;
            cancelNow = entry.cancelAfterWrite;
        }
        if (cancelNow) {
            transport.notify(Protocol.CANCEL_REQUEST, Collections.singletonMap("id", sent.id));
        }
        sent.response.whenComplete((result, error) -> {
            synchronized (lock) {
                // The slot frees when Core answers, also after a cancellation:
                // Core answers a cancelled request at once.
                inFlight--;
            }
            settle(entry, result, error == null ? null : Backend.unwrap(error));
            drain();
        });
    }

    private void cancel(Entry entry, Cancellation cancellation) {
        String id = null;
        synchronized (lock) {
            if (entry.settled) {
                return;
            }
            if (entry.state == State.QUEUED) {
                queue.remove(entry);
            } else if (entry.state == State.SENDING) {
                entry.cancelAfterWrite = true;
            } else {
                id = entry.id;
            }
        }
        if (id != null) {
            transport.notify(Protocol.CANCEL_REQUEST, Collections.singletonMap("id", id));
        }
        settle(entry, null, new CancelledException(cancellation.reason()));
    }

    /** Completes the caller's future once, on a handler thread. */
    private void settle(Entry entry, Object result, Throwable error) {
        List<Cancellation.Registration> registrations;
        synchronized (lock) {
            if (entry.settled) {
                return;
            }
            entry.settled = true;
            registrations = new ArrayList<>(entry.registrations);
            entry.registrations.clear();
        }
        for (Cancellation.Registration registration : registrations) {
            registration.remove();
        }
        Runnable complete = () -> {
            if (error != null) {
                entry.future.completeExceptionally(error);
            } else {
                entry.future.complete(result);
            }
        };
        try {
            transport.execute(complete);
        } catch (RejectedExecutionException ended) {
            complete.run();
        }
    }

    /** Queues one event; called with the lock held. */
    private void emit(Route route, long sequence, Object event) {
        boolean gap = sequence != route.lastSequence + 1;
        route.lastSequence = sequence;
        route.pending.add(new Host.Event(route.id, sequence, gap, event));
        if (route.draining) {
            return;
        }
        route.draining = true;
        try {
            transport.execute(() -> run(route));
        } catch (RejectedExecutionException ended) {
            route.draining = false;
            route.pending.clear();
        }
    }

    /** Hands a subscription's events to its listener one at a time. */
    private void run(Route route) {
        while (true) {
            Host.Event event;
            synchronized (lock) {
                event = route.live ? route.pending.poll() : null;
                if (event == null) {
                    route.draining = false;
                    route.pending.clear();
                    return;
                }
            }
            try {
                if (route.listener != null) {
                    route.listener.onEvent(event);
                }
            } catch (Throwable error) {
                transport.log("A subscription listener failed:", error);
            }
        }
    }

    private boolean open() {
        synchronized (lock) {
            return !closed && transport.ready();
        }
    }

    private static PluginSdkError notReady() {
        return new PluginSdkError("sdk.session.notReady", "The session is not initialized.");
    }

    private static HostError unavailable(String method, String message) {
        return new HostError("plugin.host.unavailable", message, Protocol.METHOD_NOT_FOUND, null, method);
    }

    private static <T> CompletableFuture<T> failed(Throwable error) {
        CompletableFuture<T> future = new CompletableFuture<>();
        future.completeExceptionally(error);
        return future;
    }
}
