//! The sender's half of thru routes (#446, see [`midi_thru`](crate::link::midi_thru)):
//! what a MIDI In passed down a route goes out on its port at once, not
//! when a sample is heard, and the notes each route holds are let go when
//! it changes or goes, as a MIDI Out's are when it's deleted.

use super::*;
use crate::link::midi_thru::{Forward, Route, ThruBus, ROUTES};

/// What sending the thru routes remembers.
#[derive(Default)]
pub(super) struct Thru {
    bus: Option<&'static ThruBus>,
    ring: Option<Consumer<Forward>>,
    /// Each route as last seen.
    routes: [Option<Route>; ROUTES],
    /// The notes each route holds on its port, a bit per note, per channel.
    held: [[u128; 16]; ROUTES],
    /// What each route has passed.
    counts: [u32; ROUTES],
}

impl Thru {
    /// Does a route play port `port`?
    pub(super) fn uses(&self, port: usize) -> bool {
        self.routes.iter().flatten().any(|route| route.port as usize == port)
    }
}

impl MidiSender {
    /// Sends what's passed down `bus`'s thru routes, too.
    pub fn with_thru(mut self, bus: &'static ThruBus) -> Self {
        bus.start_draining();
        self.thru.bus = Some(bus);
        self
    }

    /// Sends everything passed down a route since the last look, at once.
    /// Returns whether there was any. Cheap when there's nothing: the
    /// sender's thread calls it each time it's woken.
    pub fn forward(&mut self, now: Instant) -> bool {
        let Some(bus) = self.thru.bus else { return false };
        if let Some(ring) = bus.take_ring() {
            // The MIDI engine started again
            self.thru.ring = Some(ring);
        }
        self.follow_routes(bus, now);
        let Self { thru, ports, outputs, devices, .. } = self;
        let Some(ring) = thru.ring.as_mut() else { return false };
        let mut passed = false;
        while let Ok(Forward { route, port, message }) = ring.pop() {
            let index = (route as usize).min(ROUTES - 1);
            // Passed before its route moved to another port, whose notes
            // have been let go: it would only start one again
            if thru.routes[index].is_none_or(|now| now.port != port) {
                continue;
            }
            let port = &mut ports[(port as usize).min(PORTS - 1)];
            Self::send(port, outputs, devices, &mut thru.held[index], message, now, now);
            thru.counts[index] = thru.counts[index].wrapping_add(1);
            passed = true;
        }
        if passed {
            for (index, &count) in thru.counts.iter().enumerate() {
                bus.set_forwarded(index, count);
            }
        }
        passed
    }

    /// Takes up the routes as they are now, letting go of the notes of each
    /// that changed or went.
    fn follow_routes(&mut self, bus: &ThruBus, now: Instant) {
        for index in 0..ROUTES {
            let route = bus.route(index);
            let was = std::mem::replace(&mut self.thru.routes[index], route);
            if let Some(was) = was.filter(|&was| Some(was) != route) {
                let port = &mut self.ports[(was.port as usize).min(PORTS - 1)];
                Self::release(port, &mut self.outputs, &self.devices, &mut self.thru.held[index], now);
            }
        }
    }

    /// Lets go of the notes routes hold on port `port`: it's moving to
    /// another device.
    pub(super) fn release_thru(&mut self, port: usize, now: Instant) {
        let Self { thru, ports, outputs, devices, .. } = self;
        for (route, held) in thru.routes.iter().zip(&mut thru.held) {
            if route.is_some_and(|route| route.port as usize == port) {
                Self::release(&mut ports[port], outputs, devices, held, now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{sent_to, setup};
    use super::*;
    use crate::link::midi_thru::{ThruInput, ThruRoute};
    use crate::link::midi::{MidiBus, PORTS};

    /// A sender playing "Synth" on port A, with thru routes from `routes`,
    /// the input "Keys" open in slot 0.
    fn routed(routes: Vec<ThruRoute>) -> (&'static MidiBus, &'static ThruBus, MidiSender, super::super::tests::Log, ThruInput) {
        let (bus, sender, log) = setup(false);
        let thru: &'static ThruBus = Box::leak(Box::new(ThruBus::new()));
        let sender = sender.with_thru(thru);
        let input = thru.open_input().unwrap();
        thru.edit(|kept| *kept = routes);
        follow(bus, thru);
        (bus, thru, sender, log, input)
    }

    fn follow(bus: &MidiBus, thru: &ThruBus) {
        let chosen = bus.all_chosen();
        let found: [Option<String>; PORTS] = std::array::from_fn(|port| Some(chosen[port].clone()).filter(|n| !n.is_empty()));
        thru.follow(|name| (name == "Keys").then_some(0), &chosen, &found);
    }

    #[test]
    fn a_route_sends_at_once_however_late_the_audio_is() {
        let (bus, _thru, mut sender, log, mut input) = routed(vec![ThruRoute { remap: Some(4), ..ThruRoute::new("Keys", 0) }]);
        bus.set_latency(Some(Duration::from_millis(30)));
        bus.set_offset(0, 20.0);
        let start = Instant::now();
        sender.tick(start);
        input.forward(0, &[0x90, 60, 100]);
        assert!(sender.forward(start));
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!((log[0].0.as_str(), log[0].1.as_slice(), log[0].2), ("Synth", &[0x94, 60, 100][..], start), "now, not 50 ms on");
        drop(log);
        sender.tick(start + TICK);
        assert_eq!(bus.traffic(0).messages, 1, "the port's lamp flashes");
        assert_eq!(bus.status(0), PortStatus::Sending, "a route uses its port");
    }

    #[test]
    fn a_route_taken_away_lets_go_of_its_notes() {
        let (bus, thru, mut sender, log, mut input) = routed(vec![ThruRoute::new("Keys", 0)]);
        let start = Instant::now();
        sender.tick(start);
        input.forward(0, &[0x92, 60, 100]);
        input.forward(0, &[0x92, 64, 100]);
        input.forward(0, &[0x82, 64, 0]);
        sender.forward(start);
        assert_eq!(thru.forwarded(0), 3);
        thru.edit(|routes| routes.clear());
        follow(bus, thru);
        sender.forward(start + TICK);
        assert_eq!(&sent_to(&log, "Synth")[3..], [vec![0x82, 60, 0], vec![0xB2, ALL_NOTES_OFF, 0]], "only the note still held");
        // And anything passed before it went is dropped
        input.forward(0, &[0x90, 50, 100]);
        sender.forward(start + TICK * 2);
        assert_eq!(sent_to(&log, "Synth").len(), 5);
    }

    #[test]
    fn a_port_given_another_device_lets_go_of_the_routes_notes() {
        let (bus, _thru, mut sender, log, mut input) = routed(vec![ThruRoute::new("Keys", 0)]);
        let start = Instant::now();
        sender.tick(start);
        input.forward(0, &[0x90, 60, 100]);
        sender.forward(start);
        bus.choose(0, Some("DAW"));
        sender.tick(start + TICK);
        assert_eq!(sent_to(&log, "Synth"), [vec![0x90, 60, 100], vec![0x80, 60, 0], vec![0xB0, ALL_NOTES_OFF, 0]]);
    }
}
