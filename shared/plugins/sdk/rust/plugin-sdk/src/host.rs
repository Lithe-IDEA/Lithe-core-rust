//! Host services of the plugin-function protocol: the `lithe/host/*`
//! requests a backend sends to Lithe, the subscriptions some of them create,
//! and the [`Host`] handlers use. The backend owns the wire; this module owns
//! what Core expects of a well-behaved client -- at most 16 unanswered host
//! requests, cancellation through `$/cancelRequest`, and subscription events
//! routed by ID.
//!
//! Contract: `contract/v1/README.md`, "Host services" and the host sections
//! after it.

use crate::backend::{lock, Cancellation, Phase, Shared, State};
use crate::error::{sdk_codes, SdkError};
use crate::protocol::{
    generation, is_credential_key, is_function_id, is_parent_operation_id, methods,
    MAX_CREDENTIAL_SECRET_BYTES, MAX_EARLY_EVENTS, MAX_PENDING_HOST_REQUESTS,
};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

pub(crate) type Outcome = Result<Value, SdkError>;

/// The answer Core gives a host request when it did not offer `litheFunctions`.
const NOT_NEGOTIATED: &str =
    "The host did not negotiate plugin functions (litheFunctions), which offer host services.";

/// How a request this backend sent ended for its caller.
pub(crate) enum Settled {
    Answered(Outcome),
    Cancelled,
}

/// One request this backend sent to the host, until the host answers it.
pub(crate) struct Outgoing {
    method: String,
    sender: mpsc::Sender<Settled>,
    /// Whether it holds one of the 16 host-request slots. A cancelled
    /// request keeps its slot until the host answers.
    slot: bool,
    /// Where the events of the subscription it creates go.
    route: Option<Route>,
}

/// The session's host-request state, guarded by the state lock.
#[derive(Default)]
pub(crate) struct Channel {
    in_flight: usize,
    /// Requests waiting for a free slot, in call order.
    waiting: VecDeque<u64>,
    next_ticket: u64,
    /// Subscribing requests not yet answered; early events wait only while
    /// there are some.
    subscribing: usize,
    routes: HashMap<String, Route>,
    early: VecDeque<(String, u64, Value)>,
}

pub(crate) struct Route {
    live: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
    last: u64,
}

impl Route {
    fn emit(&mut self, subscription_id: &str, sequence: u64, event: Value) {
        let gap = self.last.checked_add(1) != Some(sequence);
        self.last = sequence;
        let _ = self.events.send(Event {
            subscription_id: subscription_id.to_string(),
            sequence,
            gap,
            event,
        });
    }
}

fn closed() -> SdkError {
    SdkError::new(sdk_codes::SESSION_CLOSED, "The session ended.")
}

fn cancelled() -> SdkError {
    SdkError::new(sdk_codes::CALL_CANCELLED, "The request was cancelled.")
}

/// Why `state` refuses a new request, if it does.
fn refusal(state: &State, scope: &[Cancellation]) -> Option<SdkError> {
    match state.phase {
        Phase::Ready => scope.iter().any(Cancellation::is_cancelled).then(cancelled),
        Phase::Exited => Some(closed()),
        _ => Some(SdkError::new(
            "sdk.session.notReady",
            "The session is not initialized.",
        )),
    }
}

/// Wakes a request whose scope was cancelled, waiting for a slot or an answer.
fn wake(shared: &Weak<Shared>, sender: &mpsc::Sender<Settled>) {
    let _ = sender.send(Settled::Cancelled);
    if let Some(shared) = shared.upgrade() {
        // Taking the lock orders this wake-up after the waiter's last check.
        let _state = lock(&shared.state);
        shared.slots.notify_all();
    }
}

impl Shared {
    /// Sends a request to the host and waits for its answer, the end of the
    /// session, or the cancellation of a token in `scope`. A request with a
    /// `slot` first waits its turn among the 16 the host accepts at once.
    pub(crate) fn exchange(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        slot: bool,
        scope: &[Cancellation],
        route: Option<Route>,
    ) -> Outcome {
        let id = format!("sdk-{}", self.next_request.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = mpsc::channel();
        let watches: Vec<_> = scope
            .iter()
            .filter_map(|token| {
                let shared = Arc::downgrade(self);
                let sender = sender.clone();
                let watch = token.watch(move || wake(&shared, &sender))?;
                Some((token, watch))
            })
            .collect();
        let live = route.as_ref().map(|route| Arc::clone(&route.live));
        let outcome = self.send_and_wait(&id, method, params, slot, scope, route, sender, receiver);
        for (token, watch) in watches {
            token.unwatch(watch);
        }
        match outcome {
            Settled::Answered(outcome) => outcome,
            Settled::Cancelled => {
                let pending = {
                    let mut state = lock(&self.state);
                    if let Some(live) = &live {
                        // Answered just before the cancellation won.
                        state
                            .channel
                            .routes
                            .retain(|_, route| !Arc::ptr_eq(&route.live, live));
                    }
                    state.outgoing.get_mut(&id).map(|entry| entry.route = None)
                };
                if pending.is_some() && self.functions_version().is_some() {
                    self.write(&json!({
                        "jsonrpc": "2.0",
                        "method": methods::CANCEL_REQUEST,
                        "params": { "id": id },
                    }));
                }
                // A cancelled mutation may already have taken effect; it is
                // never sent again.
                Err(cancelled())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send_and_wait(
        &self,
        id: &str,
        method: &str,
        params: Value,
        slot: bool,
        scope: &[Cancellation],
        route: Option<Route>,
        sender: mpsc::Sender<Settled>,
        receiver: mpsc::Receiver<Settled>,
    ) -> Settled {
        let mut state = lock(&self.state);
        if slot {
            let ticket = state.channel.next_ticket;
            state.channel.next_ticket += 1;
            state.channel.waiting.push_back(ticket);
            loop {
                if let Some(error) = refusal(&state, scope) {
                    state.channel.waiting.retain(|waiting| *waiting != ticket);
                    self.slots.notify_all();
                    return Settled::Answered(Err(error));
                }
                if state.channel.in_flight < MAX_PENDING_HOST_REQUESTS
                    && state.channel.waiting.front() == Some(&ticket)
                {
                    break;
                }
                state = self
                    .slots
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            state.channel.waiting.pop_front();
            state.channel.in_flight += 1;
            self.slots.notify_all();
        } else if let Some(error) = refusal(&state, scope) {
            return Settled::Answered(Err(error));
        }
        state.outgoing.insert(
            id.to_string(),
            Outgoing {
                method: method.to_string(),
                sender,
                slot,
                route,
            },
        );
        drop(state);
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.write(&message);
        receiver
            .recv()
            .unwrap_or_else(|_| Settled::Answered(Err(closed())))
    }

    /// Settles the request a host response answers.
    pub(crate) fn settle_outgoing(&self, id: &Value, message: &Map<String, Value>) {
        let Some(id) = id.as_str() else {
            return;
        };
        let mut state = lock(&self.state);
        let Some(entry) = state.outgoing.remove(id) else {
            return;
        };
        if entry.slot {
            state.channel.in_flight -= 1;
            self.slots.notify_all();
        }
        let mut outcome = match message.get("error").filter(|error| !error.is_null()) {
            Some(error) => Err(SdkError::from_response(&entry.method, error)),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        if let (Ok(result), Some(mut route)) = (&outcome, entry.route) {
            match result["subscriptionID"]
                .as_str()
                .filter(|id| is_credential_key(id))
            {
                Some(subscription_id) => {
                    let channel = &mut state.channel;
                    let (early, rest) = channel
                        .early
                        .drain(..)
                        .partition::<VecDeque<_>, _>(|(id, _, _)| id == subscription_id);
                    channel.early = rest;
                    // Events the host sent before this answer.
                    for (_, sequence, event) in early {
                        route.emit(subscription_id, sequence, event);
                    }
                    channel.routes.insert(subscription_id.to_string(), route);
                }
                None => {
                    outcome = Err(SdkError::new(
                        "sdk.host.invalidResult",
                        format!("{} answered without a subscriptionID.", entry.method),
                    ))
                }
            }
        }
        drop(state);
        let _ = entry.sender.send(Settled::Answered(outcome));
    }

    /// Routes one `lithe/host/subscriptions/notify`.
    pub(crate) fn deliver(&self, params: &Value) {
        let Some(subscription_id) = params["subscriptionID"].as_str() else {
            return;
        };
        let sequence = params["sequence"].as_u64().unwrap_or(0);
        let event = params.get("event").cloned().unwrap_or(Value::Null);
        let mut state = lock(&self.state);
        let channel = &mut state.channel;
        if let Some(route) = channel.routes.get_mut(subscription_id) {
            route.emit(subscription_id, sequence, event);
        } else if channel.subscribing > 0 {
            if channel.early.len() < MAX_EARLY_EVENTS {
                channel
                    .early
                    .push_back((subscription_id.to_string(), sequence, event));
            } else {
                drop(state);
                self.failed("notification", "subscription_queue_full");
            }
        }
        // Otherwise the subscription was disposed; nothing follows a disposal.
    }

    /// Ends every host request and subscription with the session. Runs with
    /// the state locked.
    pub(crate) fn close_channel(&self, state: &mut State) {
        for (_, entry) in state.outgoing.drain() {
            let _ = entry.sender.send(Settled::Answered(Err(closed())));
        }
        for (_, route) in state.channel.routes.drain() {
            route.live.store(false, Ordering::SeqCst);
        }
        state.channel.early.clear();
        self.slots.notify_all();
    }

    /// Stops routing a subscription's events at once and asks the host to
    /// dispose it. `live` limits the first step to one subscription object.
    fn dispose(
        self: &Arc<Self>,
        subscription_id: &str,
        live: Option<&Arc<AtomicBool>>,
    ) -> Result<(), SdkError> {
        let ready = {
            let mut state = lock(&self.state);
            let routes = &mut state.channel.routes;
            let owned = routes
                .get(subscription_id)
                .is_some_and(|route| live.is_none_or(|live| Arc::ptr_eq(&route.live, live)));
            if owned {
                if let Some(route) = routes.remove(subscription_id) {
                    route.live.store(false, Ordering::SeqCst);
                }
            }
            state.phase == Phase::Ready
        };
        if !ready {
            return Ok(());
        }
        Host::new(Arc::clone(self), Vec::new())
            .request(
                methods::SUBSCRIPTIONS_DISPOSE,
                json!({ "subscriptionID": subscription_id }),
            )
            .map(drop)
    }
}

/// Host services (`lithe/host/*`): credentials; workspace contexts, files,
/// documents, the editor, previews, the browser, messages, and capability
/// discovery; calls to other plugins' functions. They are available when the
/// host offered `litheFunctions`. Every method blocks until the host answers;
/// failures the host reports carry its `error.data.code`, such as
/// `plugin.host.contextExpired`.
///
/// [`BackendHandle::host`](crate::BackendHandle::host) is usable once the
/// session is ready. A handler's [`Context::host`](crate::Context::host)
/// belongs to its call: requests still unanswered when the call is cancelled
/// or ends fail with `sdk.call.cancelled` and are cancelled on the wire. At most 16 requests are on the wire at once; the rest wait
/// their turn.
#[derive(Clone)]
pub struct Host {
    shared: Arc<Shared>,
    scope: Vec<Cancellation>,
    /// The `operationID` of the relayed call this host serves, if any; the
    /// default `parentOperationID` of its cross-plugin calls.
    parent_operation: Option<String>,
}

impl Host {
    pub(crate) fn new(shared: Arc<Shared>, scope: Vec<Cancellation>) -> Self {
        Self {
            shared,
            scope,
            parent_operation: None,
        }
    }

    /// The same host for a call serving the relayed `lithe/function/invoke`
    /// whose `operationID` is `parent`.
    pub(crate) fn with_parent_operation(mut self, parent: Option<String>) -> Self {
        self.parent_operation = parent;
        self
    }

    /// Whether the host offered `litheFunctions`, and with it host services,
    /// in this session; `false` before `initialize`.
    pub fn negotiated(&self) -> bool {
        self.shared.functions_version().is_some()
    }

    /// The same services, with requests also cancelled by `cancellation`,
    /// for work a plugin may stop for its own reasons.
    pub fn with_cancellation(&self, cancellation: &Cancellation) -> Host {
        let mut scope = self.scope.clone();
        scope.push(cancellation.clone());
        Self::new(Arc::clone(&self.shared), scope)
            .with_parent_operation(self.parent_operation.clone())
    }

    /// Sends any `lithe/host/*` request, including methods a later host
    /// adds; [`supports`](Self::supports) says whether the session may call
    /// it. `Value::Null` sends no params.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, SdkError> {
        self.send(method, params, None)
    }

    /// Sends a request that answers `{ subscriptionID }` and routes the
    /// subscription's events to the returned [`Subscription`].
    pub fn subscribe(&self, method: &str, params: Value) -> Result<Subscription, SdkError> {
        let (events, receiver) = mpsc::channel();
        let live = Arc::new(AtomicBool::new(true));
        let route = Route {
            live: Arc::clone(&live),
            events,
            last: 0,
        };
        lock(&self.shared.state).channel.subscribing += 1;
        let result = self.send(method, params, Some(route));
        {
            let mut state = lock(&self.shared.state);
            let channel = &mut state.channel;
            channel.subscribing -= 1;
            if channel.subscribing == 0 {
                channel.early.clear();
            }
        }
        let result = result?;
        Ok(Subscription {
            id: result["subscriptionID"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            shared: Arc::clone(&self.shared),
            live,
            events: Mutex::new(receiver),
        })
    }

    /// `{ protocolVersion, backendAuthority, services, limits }`, asked once
    /// per session.
    pub fn capabilities(&self) -> Result<Value, SdkError> {
        if !self.negotiated() {
            return Err(SdkError::unavailable(NOT_NEGOTIATED));
        }
        // Held during the request, so concurrent callers share one answer.
        let mut cached = lock(&self.shared.capabilities);
        if let Some(capabilities) = &*cached {
            return Ok(capabilities.clone());
        }
        let capabilities = self.request(methods::GET_CAPABILITIES, Value::Null)?;
        if !capabilities.is_object() {
            return Err(SdkError::new(
                "sdk.host.invalidResult",
                "lithe/host/getCapabilities answered with something other than an object.",
            ));
        }
        *cached = Some(capabilities.clone());
        Ok(capabilities)
    }

    /// Whether this session may call `method`, from
    /// [`capabilities`](Self::capabilities). A missing grant still fails the
    /// request. Without `litheFunctions` nothing is supported.
    pub fn supports(&self, method: &str) -> Result<bool, SdkError> {
        if !self.negotiated() {
            return Ok(false);
        }
        let capabilities = self.capabilities()?;
        Ok(capabilities["services"]
            .as_object()
            .is_some_and(|services| {
                services.values().any(|service| {
                    service["methods"].as_array().is_some_and(|methods| {
                        methods.iter().any(|entry| entry.as_str() == Some(method))
                    })
                })
            }))
    }

    pub fn credentials(&self) -> Credentials {
        Credentials { host: self.clone() }
    }

    pub fn subscriptions(&self) -> Subscriptions {
        Subscriptions { host: self.clone() }
    }

    pub fn workspace(&self) -> Workspace {
        Workspace { host: self.clone() }
    }

    pub fn documents(&self) -> Documents {
        Documents { host: self.clone() }
    }

    pub fn editor(&self) -> Editor {
        Editor { host: self.clone() }
    }

    pub fn preview(&self) -> Preview {
        Preview { host: self.clone() }
    }

    pub fn browser(&self) -> Browser {
        Browser { host: self.clone() }
    }

    pub fn terminal(&self) -> Terminal {
        Terminal { host: self.clone() }
    }

    pub fn window(&self) -> Window {
        Window { host: self.clone() }
    }

    /// Calls to other plugins' functions through Core.
    pub fn functions(&self) -> Functions {
        Functions { host: self.clone() }
    }

    fn send(&self, method: &str, params: Value, route: Option<Route>) -> Outcome {
        if !method.starts_with(methods::HOST_PREFIX) {
            return Err(SdkError::new(
                "sdk.host.method",
                format!("{method} is not a host method; host requests are lithe/host/*."),
            ));
        }
        if let Some(error) = refusal(&lock(&self.shared.state), &[]) {
            return Err(error);
        }
        if !self.negotiated() {
            // The answer Core would give, without the round trip.
            return Err(SdkError::unavailable(NOT_NEGOTIATED));
        }
        if self.scope.iter().any(Cancellation::is_cancelled) {
            return Err(cancelled());
        }
        self.shared
            .exchange(method, params, true, &self.scope, route)
    }

    /// A method whose params name a workspace context; answers an object or
    /// `Value::Null`.
    fn scoped(&self, method: &str, context: &WorkspaceContext, params: Value) -> Outcome {
        let params = with_context(context, params)?;
        match self.request(method, params)? {
            result @ (Value::Null | Value::Object(_)) => Ok(result),
            _ => Err(SdkError::new(
                "sdk.host.invalidResult",
                format!("{method} answered with something other than an object."),
            )),
        }
    }

    /// A method that always answers an object.
    fn request_object(&self, method: &str, params: Value) -> Outcome {
        match self.request(method, params)? {
            result @ Value::Object(_) => Ok(result),
            _ => Err(SdkError::new(
                "sdk.host.invalidResult",
                format!("{method} answered with something other than an object."),
            )),
        }
    }
}

/// `params` with `context` added; params are an object or `Value::Null`.
fn with_context(context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
    let mut params = match params {
        Value::Null => Map::new(),
        Value::Object(params) => params,
        _ => {
            return Err(SdkError::new(
                "sdk.host.invalidParams",
                "Host request params are an object.",
            ))
        }
    };
    params.insert("context".into(), context.to_value());
    Ok(Value::Object(params))
}

/// A workspace context reference, `{ contextID, generation }`: the window a
/// call or command ran in, or one from `listContexts`. A context is replaced
/// when its window's folders change; requests with an old generation fail
/// with `plugin.host.contextExpired`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WorkspaceContext {
    context_id: String,
    generation: u64,
}

impl WorkspaceContext {
    /// `context_id` is 1-128 ASCII letters, digits, `.`, `_`, `:` or `-`;
    /// `generation` is positive.
    pub fn new(context_id: impl Into<String>, generation: u64) -> Result<Self, SdkError> {
        Self::from_value(&json!({ "contextID": context_id.into(), "generation": generation }))
    }

    /// The reference of a context reference or of a full context from
    /// `listContexts` or `getContext`, so either can be passed on.
    pub fn from_value(value: &Value) -> Result<Self, SdkError> {
        let context_id = value["contextID"]
            .as_str()
            .filter(|id| is_credential_key(id));
        match (context_id, generation(&value["generation"])) {
            (Some(context_id), Some(generation)) => Ok(Self {
                context_id: context_id.to_string(),
                generation,
            }),
            _ => Err(SdkError::new(
                "sdk.host.invalidContext",
                "A context is { contextID, generation }: an ID of 1-128 ASCII letters, digits, \".\", \"_\", \":\" or \"-\", and a positive generation.",
            )),
        }
    }

    /// The `context` of an invocation or command: exactly a reference.
    pub(crate) fn from_reference(value: &Value) -> Option<Self> {
        if value.as_object()?.len() != 2 {
            return None;
        }
        Self::from_value(value).ok()
    }

    pub fn context_id(&self) -> &str {
        &self.context_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// `{ "contextID": …, "generation": … }`.
    pub fn to_value(&self) -> Value {
        json!({ "contextID": self.context_id, "generation": self.generation })
    }
}

/// One `lithe/host/subscriptions/notify`.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    subscription_id: String,
    sequence: u64,
    gap: bool,
    event: Value,
}

impl Event {
    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }

    /// Starts at 1 and grows by one per event of the subscription.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// True when events before this one were lost, so state built from
    /// earlier events should be refreshed, for example by rescanning.
    pub fn gap(&self) -> bool {
        self.gap
    }

    /// The event, usually an object.
    pub fn event(&self) -> &Value {
        &self.event
    }

    pub fn into_event(self) -> Value {
        self.event
    }
}

/// A host subscription, such as a file watch or registered editor actions.
/// Events queue in order until read; dropping the subscription does not
/// dispose it on the host, [`dispose`](Self::dispose) does.
pub struct Subscription {
    id: String,
    shared: Arc<Shared>,
    live: Arc<AtomicBool>,
    events: Mutex<mpsc::Receiver<Event>>,
}

impl Subscription {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// True once disposed here or the session ended.
    pub fn is_disposed(&self) -> bool {
        !self.live.load(Ordering::SeqCst)
    }

    /// The next event, waiting for it; `None` once disposed or the session
    /// ended.
    pub fn recv(&self) -> Option<Event> {
        let event = lock(&self.events).recv().ok()?;
        (!self.is_disposed()).then_some(event)
    }

    /// The next event, waiting at most `timeout`; `Disconnected` once
    /// disposed or the session ended.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, RecvTimeoutError> {
        if self.is_disposed() {
            return Err(RecvTimeoutError::Disconnected);
        }
        let event = lock(&self.events).recv_timeout(timeout)?;
        if self.is_disposed() {
            Err(RecvTimeoutError::Disconnected)
        } else {
            Ok(event)
        }
    }

    /// Stops routing events at once and asks the host to dispose the
    /// subscription. After the session ended there is nothing to ask.
    pub fn dispose(&self) -> Result<(), SdkError> {
        self.shared.dispose(&self.id, Some(&self.live))
    }
}

/// The host's `credentials` service: secrets private to this
/// plugin, kept in the platform's secret store, never in `${stateDir}`. It
/// needs the `credentials.store` permission; without it every request fails
/// with `plugin.host.permissionDenied`.
pub struct Credentials {
    host: Host,
}

impl Credentials {
    /// The secret stored under `key`, or `None`.
    pub fn get(&self, key: &str) -> Result<Option<String>, SdkError> {
        check_credential_key(key)?;
        match self
            .host
            .request(methods::CREDENTIALS_GET, json!({ "key": key }))?
        {
            Value::Null => Ok(None),
            Value::Object(mut result) => match result.remove("secret") {
                Some(Value::String(secret)) => Ok(Some(secret)),
                _ => Err(missing_secret()),
            },
            _ => Err(missing_secret()),
        }
    }

    /// Stores `secret` (at most 16 KiB) under `key`, replacing any value.
    pub fn set(&self, key: &str, secret: &str) -> Result<(), SdkError> {
        check_credential_key(key)?;
        if secret.len() > MAX_CREDENTIAL_SECRET_BYTES {
            return Err(SdkError::new(
                "sdk.credentials.invalidSecret",
                "A secret is a string of at most 16 KiB.",
            ));
        }
        self.host
            .request(
                methods::CREDENTIALS_SET,
                json!({ "key": key, "secret": secret }),
            )
            .map(drop)
    }

    /// Removes the secret under `key`; succeeds when nothing was stored.
    pub fn delete(&self, key: &str) -> Result<(), SdkError> {
        check_credential_key(key)?;
        self.host
            .request(methods::CREDENTIALS_DELETE, json!({ "key": key }))
            .map(drop)
    }
}

fn check_credential_key(key: &str) -> Result<(), SdkError> {
    if is_credential_key(key) {
        Ok(())
    } else {
        Err(SdkError::new(
            "sdk.credentials.invalidKey",
            "A credential key is 1-128 ASCII letters, digits, \".\", \"_\", \":\" or \"-\".",
        ))
    }
}

fn missing_secret() -> SdkError {
    SdkError::new(
        "sdk.host.invalidResult",
        "The host answered credentials/get without a secret.",
    )
}

pub struct Subscriptions {
    host: Host,
}

impl Subscriptions {
    /// Disposes a subscription by ID; also succeeds for an unknown or
    /// disposed one.
    pub fn dispose(&self, subscription_id: &str) -> Result<(), SdkError> {
        self.host.shared.dispose(subscription_id, None)
    }
}

/// Workspace contexts and files. Every method but
/// [`list_contexts`](Self::list_contexts) names a context: the call's
/// [`Context::workspace_context`](crate::Context::workspace_context), or one
/// from `list_contexts` read with [`WorkspaceContext::from_value`].
pub struct Workspace {
    host: Host,
}

impl Workspace {
    /// `{ revision, contexts }`.
    pub fn list_contexts(&self) -> Result<Value, SdkError> {
        self.host
            .scoped_without_context(methods::WORKSPACE_LIST_CONTEXTS)
    }

    pub fn get_context(&self, context: &WorkspaceContext) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WORKSPACE_GET_CONTEXT, context, Value::Null)
    }

    /// `{ folders }`.
    pub fn get_folders(&self, context: &WorkspaceContext) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WORKSPACE_GET_FOLDERS, context, Value::Null)
    }

    /// `params`: `resource, cursor?, limit?`.
    pub fn read_directory(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WORKSPACE_READ_DIRECTORY, context, params)
    }

    /// `params`: `resource`.
    pub fn stat(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host.scoped(methods::WORKSPACE_STAT, context, params)
    }

    /// `params`: `resource, maximumBytes?`.
    pub fn read_file(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WORKSPACE_READ_FILE, context, params)
    }

    /// Watches `resource, recursive?`; events are
    /// `{ rootID, changedPaths, requiresRescan }`.
    pub fn watch(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Subscription, SdkError> {
        self.host
            .subscribe(methods::WORKSPACE_WATCH, with_context(context, params)?)
    }

    /// `params`: root `resource, query, fileMask?, cursor?, limit?`.
    pub fn find_files(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WORKSPACE_FIND_FILES, context, params)
    }

    /// `params`: root `resource, pattern`, and optional search options.
    pub fn find_text(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WORKSPACE_FIND_TEXT, context, params)
    }
}

impl Host {
    fn scoped_without_context(&self, method: &str) -> Outcome {
        match self.request(method, json!({}))? {
            result @ (Value::Null | Value::Object(_)) => Ok(result),
            _ => Err(SdkError::new(
                "sdk.host.invalidResult",
                format!("{method} answered with something other than an object."),
            )),
        }
    }
}

/// Open documents, read from their live buffers.
pub struct Documents {
    host: Host,
}

impl Documents {
    /// `{ documents }`.
    pub fn list(&self, context: &WorkspaceContext) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::DOCUMENTS_LIST, context, Value::Null)
    }

    /// `params`: `documentID`; answers `{ document, text }`.
    pub fn get_snapshot(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::DOCUMENTS_GET_SNAPSHOT, context, params)
    }
}

/// The text editor.
pub struct Editor {
    host: Host,
}

impl Editor {
    /// The foreground text tab's document summary, or `Value::Null`.
    pub fn get_active(&self, context: &WorkspaceContext) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::EDITOR_GET_ACTIVE, context, Value::Null)
    }

    /// `params`: `documentID`.
    pub fn get_selections(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::EDITOR_GET_SELECTIONS, context, params)
    }

    /// `params`: `documentID, expectedVersion, edits`.
    pub fn apply_edits(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::EDITOR_APPLY_EDITS, context, params)
    }

    /// `params`: `documentID, expectedVersion, selections, replacements`.
    pub fn replace_selections(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::EDITOR_REPLACE_SELECTIONS, context, params)
    }

    /// `params`: `documentID, expectedVersion, at, text`.
    pub fn insert_text(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::EDITOR_INSERT_TEXT, context, params)
    }

    /// `params`: `resource, activation?, range?, tab?`.
    pub fn open(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host.scoped(methods::EDITOR_OPEN, context, params)
    }

    /// `params`: `resource, activation?, range?, tab?`.
    pub fn reveal(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host.scoped(methods::EDITOR_REVEAL, context, params)
    }

    /// Registers editor actions (`id, function, selector`) that invoke one
    /// of the backend's functions; disposing the subscription removes them.
    pub fn register_actions(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Subscription, SdkError> {
        self.host.subscribe(
            methods::EDITOR_REGISTER_ACTIONS,
            with_context(context, params)?,
        )
    }
}

/// HTML previews.
pub struct Preview {
    host: Host,
}

impl Preview {
    /// `params`: HTML `resource, presentation?, range?`.
    pub fn open_html(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::PREVIEW_OPEN_HTML, context, params)
    }
}

/// The system browser.
pub struct Browser {
    host: Host,
}

impl Browser {
    /// `params`: exactly one of an HTTP(S) `url` or an HTML `resource`.
    pub fn open_external(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::BROWSER_OPEN_EXTERNAL, context, params)
    }
}

/// Messages in the window. `params`: `text, actions?`; answers
/// `{ selectedIndex }`, `Value::Null` on dismissal.
pub struct Window {
    host: Host,
}

impl Window {
    pub fn show_information(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WINDOW_SHOW_INFORMATION, context, params)
    }

    pub fn show_warning(
        &self,
        context: &WorkspaceContext,
        params: Value,
    ) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WINDOW_SHOW_WARNING, context, params)
    }

    pub fn show_error(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host
            .scoped(methods::WINDOW_SHOW_ERROR, context, params)
    }
}

/// Integrated native terminals owned by this backend session and workspace.
/// Permission `terminal.create`; discover each method with `Host::supports`.
/// Input and output stay in the host terminal, not in the plugin's web page.
pub struct Terminal {
    host: Host,
}

impl Terminal {
    /// Launches executable and arguments directly. Params: `executable`, optional
    /// `arguments`, `title`, `cwd` resource, `environment`, and `show` (true).
    pub fn create(&self, context: &WorkspaceContext, params: Value) -> Result<Value, SdkError> {
        self.host.scoped(methods::TERMINAL_CREATE, context, params)
    }

    /// Shows an owned terminal in its original window without raising the window.
    pub fn show(&self, context: &WorkspaceContext, terminal_id: &str) -> Result<Value, SdkError> {
        self.host.scoped(
            methods::TERMINAL_SHOW,
            context,
            json!({ "terminalID": terminal_id }),
        )
    }

    /// Stops the native process, keeping a bounded display suffix for reopen.
    /// `retained_context_bytes` overrides the create-time choice (0..=262144).
    pub fn close(
        &self,
        context: &WorkspaceContext,
        terminal_id: &str,
        retained_context_bytes: Option<usize>,
    ) -> Result<Value, SdkError> {
        let mut params = json!({ "terminalID": terminal_id });
        if let Some(bytes) = retained_context_bytes {
            params["retainedContextBytes"] = json!(bytes);
        }
        self.host.scoped(methods::TERMINAL_CLOSE, context, params)
    }

    /// Starts the saved argv again and restores plain display history, never input.
    pub fn reopen(&self, context: &WorkspaceContext, terminal_id: &str) -> Result<Value, SdkError> {
        self.host.scoped(
            methods::TERMINAL_REOPEN,
            context,
            json!({ "terminalID": terminal_id }),
        )
    }

    /// Permanently forgets an owned handle and its history, also closing it if live.
    pub fn dispose(
        &self,
        context: &WorkspaceContext,
        terminal_id: &str,
    ) -> Result<Value, SdkError> {
        self.host.scoped(
            methods::TERMINAL_DISPOSE,
            context,
            json!({ "terminalID": terminal_id }),
        )
    }

    /// One exit event per process generation, including the latest recorded
    /// outcome when subscribing after exit.
    /// Disposing the subscription stops notifications, not the terminal itself.
    pub fn on_exit(
        &self,
        context: &WorkspaceContext,
        terminal_id: &str,
    ) -> Result<Subscription, SdkError> {
        self.host.subscribe(
            methods::TERMINAL_ON_EXIT,
            with_context(context, json!({ "terminalID": terminal_id }))?,
        )
    }
}

/// Calls to the functions other plugins export, relayed by Core.
/// Needs a call the plugin declares as `pluginFunctions.calls` in its
/// manifest; discover the methods with `Host::supports`. The relay accepts
/// local workspaces only.
pub struct Functions {
    host: Host,
}

impl Functions {
    /// `{ providers }`: the enabled plugins other than this one that export
    /// `function_id`, sorted by plugin ID, each `{ pluginID, version }`. This
    /// plugin must have declared the function.
    pub fn providers(&self, function_id: &str) -> Result<Value, SdkError> {
        if !is_function_id(function_id) {
            return Err(invalid_call("functionID is a plugin function ID."));
        }
        self.host.request_object(
            methods::FUNCTIONS_PROVIDERS,
            json!({ "functionID": function_id }),
        )
    }

    /// Calls one provider and answers `{ value }`, the provider's result.
    /// `call` is an object with `pluginID` and `functionID`, and optionally
    /// `params` (an object), `timeoutMilliseconds` (a positive integer; the
    /// host defaults it and caps it at 600000), and `parentOperationID`; any
    /// other key is not sent. `context` is a workspace context this backend
    /// learned from the workspace service.
    ///
    /// Inside a handler, `parentOperationID` defaults to the `operationID`
    /// of the call the handler serves, which tells Core the new call belongs
    /// to it. A [`BackendHandle::host`](crate::BackendHandle::host) serves no
    /// call, so it sends the field only when `call` sets it. Cancelling the
    /// handler's call cancels the relayed call.
    pub fn invoke(&self, context: &WorkspaceContext, call: Value) -> Result<Value, SdkError> {
        let call = relay_params(call, self.host.parent_operation.as_deref())?;
        let params = with_context(context, call)?;
        self.host.request_object(methods::FUNCTIONS_INVOKE, params)
    }
}

fn invalid_call(message: &str) -> SdkError {
    SdkError::new("sdk.functions.invalidCall", message)
}

/// The params of `lithe/host/functions/invoke` for `call`, without its
/// context: the fields the method defines and no other, so the host never
/// answers `-32018`. `serving` is the `operationID` that `parentOperationID`
/// defaults to.
fn relay_params(call: Value, serving: Option<&str>) -> Result<Value, SdkError> {
    let Value::Object(mut call) = call else {
        return Err(invalid_call("A cross-plugin call is an object."));
    };
    let mut relayed = Map::new();
    match call.remove("pluginID") {
        Some(Value::String(plugin_id)) if !plugin_id.is_empty() => {
            relayed.insert("pluginID".into(), Value::String(plugin_id));
        }
        _ => return Err(invalid_call("pluginID is a non-empty string.")),
    }
    match call.remove("functionID") {
        Some(Value::String(function_id)) if is_function_id(&function_id) => {
            relayed.insert("functionID".into(), Value::String(function_id));
        }
        _ => return Err(invalid_call("functionID is a plugin function ID.")),
    }
    match call.remove("params") {
        Some(params @ Value::Object(_)) => {
            relayed.insert("params".into(), params);
        }
        Some(_) => return Err(invalid_call("params is an object.")),
        None => {}
    }
    match call.remove("timeoutMilliseconds") {
        Some(timeout) if timeout.as_u64().is_some_and(|ms| ms > 0) => {
            relayed.insert("timeoutMilliseconds".into(), timeout);
        }
        Some(_) => return Err(invalid_call("timeoutMilliseconds is a positive integer.")),
        None => {}
    }
    match call.remove("parentOperationID") {
        Some(Value::String(parent)) if is_parent_operation_id(&parent) => {
            relayed.insert("parentOperationID".into(), Value::String(parent));
        }
        Some(_) => return Err(invalid_call("parentOperationID is a 1-128 byte string.")),
        None => {
            if let Some(parent) = serving {
                relayed.insert("parentOperationID".into(), json!(parent));
            }
        }
    }
    Ok(Value::Object(relayed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_are_references_or_full_contexts() {
        let full = json!({ "contextID": "w1", "generation": 2, "name": "orders" });
        let context = WorkspaceContext::from_value(&full).unwrap();
        assert_eq!(
            context.to_value(),
            json!({ "contextID": "w1", "generation": 2 })
        );
        assert_eq!(WorkspaceContext::new("w1", 2).unwrap(), context);
        // An invocation's context is exactly a reference.
        assert_eq!(WorkspaceContext::from_reference(&full), None);
        assert_eq!(
            WorkspaceContext::from_reference(&context.to_value()),
            Some(context)
        );
        for invalid in [
            json!({ "contextID": "w1" }),
            json!({ "contextID": "w1", "generation": 0 }),
            json!({ "contextID": "../w", "generation": 1 }),
            json!(null),
        ] {
            let error = WorkspaceContext::from_value(&invalid).unwrap_err();
            assert_eq!(error.code(), "sdk.host.invalidContext", "{invalid}");
        }
        assert!(WorkspaceContext::new("", 1).is_err());
    }

    #[test]
    fn a_relayed_call_keeps_only_the_fields_the_method_defines() {
        let call = json!({
            "pluginID": "dev.lithe.fixture.java", "functionID": "project.describe",
            "params": { "caller": "data" }, "timeoutMilliseconds": 900000,
            "parentOperationID": "17", "caller": { "pluginID": "forged" },
            "context": { "contextID": "x" }, "extra": true
        });
        let expected = json!({
            "pluginID": "dev.lithe.fixture.java", "functionID": "project.describe",
            "params": { "caller": "data" }, "timeoutMilliseconds": 900000,
            "parentOperationID": "17"
        });
        // Core passes a `caller` inside `params` unchanged; only a top-level one is refused.
        assert_eq!(relay_params(call, Some("served")).unwrap(), expected);
    }

    #[test]
    fn optional_fields_stay_out_and_the_parent_defaults_to_the_call_being_served() {
        let call = json!({ "pluginID": "a.b", "functionID": "x.y" });
        assert_eq!(relay_params(call.clone(), None).unwrap(), call);
        let served = relay_params(call, Some("op-7")).unwrap();
        assert_eq!(served["parentOperationID"], "op-7");
        assert!(served.get("params").is_none());
        assert!(served.get("timeoutMilliseconds").is_none());
        let named = json!({ "pluginID": "a.b", "functionID": "x.y", "parentOperationID": "own" });
        let relayed = relay_params(named, Some("op-7")).unwrap();
        assert_eq!(relayed["parentOperationID"], "own");
        // The host caps the timeout; the SDK sends what the plugin chose.
        let mut slow = json!({ "pluginID": "a.b", "functionID": "x.y" });
        slow["timeoutMilliseconds"] = json!(700000);
        let relayed = relay_params(slow, None).unwrap();
        assert_eq!(relayed["timeoutMilliseconds"], 700000);
    }

    #[test]
    fn an_invalid_relayed_call_is_refused_before_anything_is_sent() {
        let valid = json!({ "pluginID": "a.b", "functionID": "x.y" });
        let with = |key: &str, value: Value| {
            let mut call = valid.clone();
            call[key] = value;
            call
        };
        let without = |key: &str| {
            let mut call = valid.clone();
            call.as_object_mut().unwrap().remove(key);
            call
        };
        for invalid in [
            json!("x.y"),
            json!(null),
            without("pluginID"),
            with("pluginID", json!("")),
            with("pluginID", json!(7)),
            without("functionID"),
            with("functionID", json!("Not-Valid")),
            with("params", json!([])),
            with("params", json!(null)),
            with("timeoutMilliseconds", json!(0)),
            with("timeoutMilliseconds", json!(-1)),
            with("timeoutMilliseconds", json!(1.5)),
            with("timeoutMilliseconds", json!("5000")),
            with("parentOperationID", json!("")),
            with("parentOperationID", json!("p".repeat(129))),
            with("parentOperationID", json!(12)),
        ] {
            let error = relay_params(invalid.clone(), Some("op-1")).unwrap_err();
            assert_eq!(error.code(), "sdk.functions.invalidCall", "{invalid}");
        }
    }
}
