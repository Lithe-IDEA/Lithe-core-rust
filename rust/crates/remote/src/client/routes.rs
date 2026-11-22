//! Where the session's reader delivers each event.
//!
//! An event of a resource goes to the resource's subscriber. Without one, it
//! goes to the session's general event channel, unless an earlier connection
//! to the same persistent session created the resource and its family is
//! parked: then the event waits here until a caller attaches to the resource.
//! A reconnecting client resumes the event stream once and attaches to each
//! resource it still shows afterwards, so the events replayed for a running
//! process and its live output reach one owner without a gap or a repeat.
//! Parking and attaching share the reader's lock, so each event is either
//! among the parked ones an attach takes or in its subscription.
//!
//! Parked output is bounded per resource and in total. The oldest output goes
//! first, and the attach reports the loss. Other events, such as an exit, are
//! small and never dropped.

use super::session::{lock, Event};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Mutex;

/// Parked bytes of one resource before its oldest output is dropped.
const RESOURCE_LIMIT_BYTES: usize = 4 * 1024 * 1024;

/// Parked bytes of every resource together before output is dropped.
const TOTAL_LIMIT_BYTES: usize = 16 * 1024 * 1024;

pub(crate) type Routes = Mutex<RouteTable>;

/// A resource: its family and id.
type Key = (String, u64);

/// Subscriptions, and the events parked for resources nobody receives yet.
#[derive(Default)]
pub(crate) struct RouteTable {
    subscribers: HashMap<Key, SyncSender<Event>>,
    /// Families whose resources from earlier connections are parked.
    families: HashSet<String>,
    /// Resource ids below this were created by earlier connections.
    parking_below: u64,
    parked: HashMap<Key, Parked>,
    parked_bytes: usize,
    /// Resources an attach took; their later events are never parked again.
    attached: HashSet<Key>,
    /// Set once the stream ended.
    closed: bool,
}

/// The events of one resource, oldest first, with their sizes on the wire.
#[derive(Default)]
struct Parked {
    events: VecDeque<(Event, usize)>,
    bytes: usize,
    /// Whether output was dropped to stay within the limits.
    lost: bool,
}

/// The events of a resource that an earlier connection created, from the
/// moment this connection resumed the event stream.
#[derive(Debug)]
pub struct Attachment {
    /// Events that arrived before the attach, oldest first.
    pub missed: Vec<Event>,
    /// Whether output among `missed` was dropped to stay within the parking
    /// limits.
    pub lost: bool,
    /// Later events, like a subscription's. Dropping the receiver ends it.
    pub events: Receiver<Event>,
}

/// Why an attach took nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachRefusal {
    /// Another caller receives the resource's events: the subscriber that
    /// created it on this connection, or an earlier attach.
    Owned,
    /// Nothing keeps the resource's events for an attach: its family is not
    /// parked, this connection created it and nobody receives it any more,
    /// or an earlier attach took its events and stopped receiving.
    Unavailable,
}

impl RouteTable {
    pub(crate) fn subscribe(&mut self, family: &str, id: u64, capacity: usize) -> Receiver<Event> {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        // A closed stream drops the sender: the receiver reports at once that
        // nothing will arrive.
        if !self.closed {
            self.subscribers.insert((family.to_owned(), id), sender);
        }
        receiver
    }

    pub(crate) fn unsubscribe(&mut self, family: &str, id: u64) {
        self.subscribers.remove(&(family.to_owned(), id));
    }

    /// Parks the events of `family` resources with ids below `below`.
    pub(crate) fn park(&mut self, family: &str, below: u64) {
        self.families.insert(family.to_owned());
        self.parking_below = below;
    }

    /// Takes the parked events of a resource and subscribes to its later ones.
    pub(crate) fn attach(
        &mut self,
        family: &str,
        id: u64,
        capacity: usize,
    ) -> Result<Attachment, AttachRefusal> {
        let key = (family.to_owned(), id);
        if self.subscribers.contains_key(&key) {
            return Err(AttachRefusal::Owned);
        }
        if !self.families.contains(family)
            || id >= self.parking_below
            || self.attached.contains(&key)
        {
            return Err(AttachRefusal::Unavailable);
        }
        let parked = self.parked.remove(&key).unwrap_or_default();
        self.parked_bytes -= parked.bytes;
        let events = self.subscribe(family, id, capacity);
        self.attached.insert(key);
        Ok(Attachment {
            missed: parked.events.into_iter().map(|(event, _)| event).collect(),
            lost: parked.lost,
            events,
        })
    }

    /// Ends every subscription, which tells their consumers that the stream
    /// is gone, and discards the parked events. Returns whether there were
    /// any.
    pub(crate) fn close(&mut self) -> bool {
        self.closed = true;
        self.subscribers.clear();
        self.parked_bytes = 0;
        let discarded = !self.parked.is_empty();
        self.parked.clear();
        discarded
    }

    fn parks(&self, key: &Key) -> bool {
        key.1 < self.parking_below && self.families.contains(&key.0) && !self.attached.contains(key)
    }

    fn park_event(&mut self, key: Key, event: Event, size: usize) {
        let parked = self.parked.entry(key).or_default();
        parked.events.push_back((event, size));
        parked.bytes += size;
        self.parked_bytes += size;
        while parked.bytes > RESOURCE_LIMIT_BYTES || self.parked_bytes > TOTAL_LIMIT_BYTES {
            let Some(oldest) = parked
                .events
                .iter()
                .position(|(event, _)| event.event.ends_with(".output"))
            else {
                break;
            };
            let (_, dropped) = parked.events.remove(oldest).expect("position is in range");
            parked.bytes -= dropped;
            self.parked_bytes -= dropped;
            parked.lost = true;
        }
    }
}

/// Delivers `event`, `size` bytes on the wire. Returns `false` when the
/// connection must resynchronize: a live subscriber or the general channel was
/// full.
pub(crate) fn route(
    routes: &Routes,
    events: &SyncSender<Event>,
    event: Event,
    size: usize,
) -> bool {
    let key = event.resource().map(|(family, id)| (family.to_owned(), id));
    // Neither a resource nor the general consumer may block the sole response
    // reader: a process owner needs that reader to acknowledge its kill request.
    let subscriber = match &key {
        Some(key) => {
            let mut table = lock(routes);
            match table.subscribers.get(key).cloned() {
                Some(subscriber) => Some(subscriber),
                None if table.parks(key) => {
                    table.park_event(key.clone(), event, size);
                    return true;
                }
                None => None,
            }
        }
        None => None,
    };
    match subscriber {
        Some(subscriber) => {
            match subscriber.try_send(event) {
                Ok(()) => true,
                Err(error) => {
                    if let Some(key) = key {
                        lock(routes).subscribers.remove(&key);
                    }
                    // A live owner losing a frame must resynchronize. Dropping
                    // a receiver intentionally only ends that subscription.
                    matches!(error, mpsc::TrySendError::Disconnected(_))
                }
            }
        }
        // A dropped general receiver means nobody consumes these events.
        None => !matches!(events.try_send(event), Err(mpsc::TrySendError::Full(_))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn output(id: u64, seq: u64) -> Event {
        Event {
            event: "process.output".into(),
            params: json!({"processId": id, "data": ""}),
            seq: Some(seq),
        }
    }

    fn exit(id: u64) -> Event {
        Event {
            event: "process.exit".into(),
            params: json!({"processId": id, "code": 0}),
            seq: None,
        }
    }

    #[test]
    fn full_resource_queue_requires_resynchronization_without_blocking_replies() {
        let routes = Routes::default();
        let full = lock(&routes).subscribe("process", 7, 1);
        let healthy = lock(&routes).subscribe("process", 8, 1);
        let (general, _) = mpsc::sync_channel(1);
        assert!(route(&routes, &general, output(7, 1), 1));
        assert!(!route(&routes, &general, output(7, 2), 1));
        assert!(!lock(&routes)
            .subscribers
            .contains_key(&("process".into(), 7)));
        assert!(route(&routes, &general, output(8, 3), 1));
        assert_eq!(healthy.try_recv().unwrap().resource(), Some(("process", 8)));
        assert!(full.try_recv().is_ok());
        assert!(matches!(
            full.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn general_overflow_requires_resynchronization_instead_of_losing_watch_changes() {
        let routes = Routes::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        let event = || Event {
            event: "watch.changed".into(),
            params: json!({}),
            seq: None,
        };
        assert!(route(&routes, &sender, event(), 1));
        assert!(!route(&routes, &sender, event(), 1));
        drop(receiver);
        assert!(route(&routes, &sender, event(), 1));
    }

    #[test]
    fn dropped_resource_receiver_does_not_close_the_connection() {
        let routes = Routes::default();
        let receiver = lock(&routes).subscribe("process", 7, 1);
        let (general, _) = mpsc::sync_channel(1);
        drop(receiver);
        assert!(route(&routes, &general, output(7, 1), 1));
        assert!(!lock(&routes)
            .subscribers
            .contains_key(&("process".into(), 7)));
    }

    #[test]
    fn attach_receives_the_parked_events_then_the_live_ones_without_a_gap() {
        let routes = Routes::default();
        lock(&routes).park("process", 10);
        let (general, unrouted) = mpsc::sync_channel(4);
        assert!(route(&routes, &general, output(3, 1), 1));
        assert!(route(&routes, &general, output(3, 2), 1));
        let attached = lock(&routes).attach("process", 3, 4).unwrap();
        assert!(route(&routes, &general, exit(3), 1));
        assert_eq!(
            attached
                .missed
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            [Some(1), Some(2)]
        );
        assert!(!attached.lost);
        assert_eq!(attached.events.try_recv().unwrap().event, "process.exit");
        // Nothing of the parked resource reached the general channel.
        assert!(unrouted.try_recv().is_err());
        assert_eq!(lock(&routes).parked_bytes, 0);
    }

    #[test]
    fn only_resources_of_earlier_connections_in_a_parked_family_are_parked() {
        let routes = Routes::default();
        lock(&routes).park("process", 10);
        let (general, unrouted) = mpsc::sync_channel(4);
        // Created by this connection: its owner subscribed and has finished.
        assert!(route(&routes, &general, exit(12), 1));
        let terminal = Event {
            event: "terminal.output".into(),
            params: json!({"terminalId": 3, "data": ""}),
            seq: None,
        };
        assert!(route(&routes, &general, terminal, 1));
        assert_eq!(unrouted.try_iter().count(), 2);
        assert_eq!(
            lock(&routes).attach("process", 12, 4).unwrap_err(),
            AttachRefusal::Unavailable
        );
        assert_eq!(
            lock(&routes).attach("terminal", 3, 4).unwrap_err(),
            AttachRefusal::Unavailable
        );
    }

    #[test]
    fn a_resource_has_one_owner() {
        let routes = Routes::default();
        lock(&routes).park("process", 10);
        let (general, unrouted) = mpsc::sync_channel(4);
        let first = lock(&routes).attach("process", 3, 4).unwrap();
        assert_eq!(
            lock(&routes).attach("process", 3, 4).unwrap_err(),
            AttachRefusal::Owned
        );
        drop(first);
        // The owner stopped receiving: this event ends its subscription, and
        // the later ones are not parked again for a second attach.
        assert!(route(&routes, &general, output(3, 1), 1));
        assert!(route(&routes, &general, output(3, 2), 1));
        assert_eq!(
            lock(&routes).attach("process", 3, 4).unwrap_err(),
            AttachRefusal::Unavailable
        );
        assert_eq!(unrouted.try_iter().count(), 1);
    }

    #[test]
    fn parking_drops_the_oldest_output_but_keeps_the_exit() {
        let routes = Routes::default();
        lock(&routes).park("process", 10);
        let (general, _) = mpsc::sync_channel(1);
        let half = RESOURCE_LIMIT_BYTES / 2;
        for seq in 1..=3 {
            assert!(route(&routes, &general, output(3, seq), half));
        }
        assert!(route(&routes, &general, exit(3), half));
        let attached = lock(&routes).attach("process", 3, 4).unwrap();
        assert!(attached.lost);
        assert_eq!(
            attached
                .missed
                .iter()
                .map(|event| (event.event.as_str(), event.seq))
                .collect::<Vec<_>>(),
            [("process.output", Some(3)), ("process.exit", None)]
        );
    }

    #[test]
    fn parking_stays_within_the_total_limit() {
        let routes = Routes::default();
        lock(&routes).park("process", 100);
        let (general, _) = mpsc::sync_channel(1);
        let size = RESOURCE_LIMIT_BYTES / 2;
        for id in 1..=40 {
            assert!(route(&routes, &general, output(id, id), size));
        }
        assert!(lock(&routes).parked_bytes <= TOTAL_LIMIT_BYTES);
        let last = lock(&routes).attach("process", 40, 4).unwrap();
        assert!(last.lost);
        let first = lock(&routes).attach("process", 1, 4).unwrap();
        assert!(!first.lost);
        assert_eq!(first.missed.len(), 1);
    }

    #[test]
    fn closing_ends_subscriptions_and_reports_discarded_events() {
        let routes = Routes::default();
        lock(&routes).park("process", 10);
        let (general, _) = mpsc::sync_channel(1);
        let subscribed = lock(&routes).subscribe("process", 12, 4);
        assert!(!lock(&routes).close());
        assert!(matches!(
            subscribed.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));

        let routes = Routes::default();
        lock(&routes).park("process", 10);
        assert!(route(&routes, &general, output(3, 1), 1));
        assert!(lock(&routes).close());
        let late = lock(&routes).attach("process", 3, 4).unwrap();
        assert!(late.missed.is_empty());
        assert!(matches!(
            late.events.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
