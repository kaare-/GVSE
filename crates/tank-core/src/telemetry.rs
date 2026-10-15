//! Minimal telemetry queue. The core records; the shell posts.
//! No clock, no filesystem, no network. Consent defaults off.
//! A kill switch drops everything, including events already queued.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelEvent {
    pub kind: u32,
    pub arg0: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Telemetry {
    consent: bool,
    killed: bool,
    queue: Vec<TelEvent>,
}

impl Telemetry {
    pub fn set_consent(&mut self, on: bool) {
        self.consent = on;
        if !on {
            self.queue.clear();
        }
    }

    pub fn consent(&self) -> bool {
        self.consent
    }

    pub fn set_killed(&mut self, killed: bool) {
        self.killed = killed;
        if killed {
            self.queue.clear();
        }
    }

    pub fn killed(&self) -> bool {
        self.killed
    }

    /// Returns false when the event is discarded.
    pub fn push(&mut self, kind: u32, arg0: u32) -> bool {
        if !self.consent || self.killed {
            return false;
        }
        if self.queue.len() >= 256 {
            self.queue.remove(0);
        }
        self.queue.push(TelEvent { kind, arg0 });
        true
    }

    pub fn drain(&mut self, out: &mut Vec<TelEvent>) {
        self.drain_max(usize::MAX, out);
    }

    pub fn drain_max(&mut self, max: usize, out: &mut Vec<TelEvent>) {
        if !self.consent || self.killed {
            self.queue.clear();
            return;
        }
        let n = max.min(self.queue.len());
        out.extend(self.queue.drain(..n));
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// JSON array the shell can POST. `anon` is the shell's anonymous id;
/// the core never mints one.
pub fn format_batch(events: &[TelEvent], anon: &str) -> String {
    let mut body = String::from("{\"anon\":");
    body.push('"');
    for ch in anon.chars() {
        match ch {
            '"' | '\\' => {
                body.push('\\');
                body.push(ch);
            }
            c if c.is_control() => body.push(' '),
            c => body.push(c),
        }
    }
    body.push_str("\",\"events\":[");
    for (i, ev) in events.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        body.push_str("{\"kind\":");
        body.push_str(&ev.kind.to_string());
        body.push_str(",\"arg0\":");
        body.push_str(&ev.arg0.to_string());
        body.push('}');
    }
    body.push_str("]}");
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_off_discards_and_drain_is_empty() {
        let mut t = Telemetry::default();
        assert!(!t.push(1, 2));
        let mut out = Vec::new();
        t.drain(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn consent_on_queues_and_kill_switch_drops() {
        let mut t = Telemetry::default();
        t.set_consent(true);
        assert!(t.push(7, 9));
        assert_eq!(t.len(), 1);
        t.set_killed(true);
        assert!(t.is_empty());
        assert!(!t.push(1, 1));
        t.set_killed(false);
        assert!(t.push(3, 4));
        let mut out = Vec::new();
        t.drain(&mut out);
        assert_eq!(out, vec![TelEvent { kind: 3, arg0: 4 }]);
        assert!(t.is_empty());
    }

    #[test]
    fn batch_json_contains_the_event() {
        let events = [TelEvent { kind: 4, arg0: 8 }];
        let json = format_batch(&events, "anon-1");
        assert!(json.contains("\"kind\":4"));
        assert!(json.contains("\"arg0\":8"));
        assert!(json.contains("anon-1"));
        assert!(format_batch(&[], "x").contains("\"events\":[]"));
    }
}
