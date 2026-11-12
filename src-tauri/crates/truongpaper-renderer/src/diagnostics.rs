//! Short, deduplicated, capped list of things a scene could not draw as authored.

/// Most diagnostics kept per scene.
pub const MAX_DIAGNOSTICS: usize = 50;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct DiagnosticsLog {
    items: Vec<String>,
}

impl DiagnosticsLog {
    /// Adds `msg` unless it is empty, already present or the cap is reached.
    pub fn push(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if msg.is_empty() || self.items.len() >= MAX_DIAGNOSTICS || self.items.contains(&msg) {
            return;
        }
        self.items.push(msg);
    }

    pub fn extend<I: IntoIterator<Item = String>>(&mut self, msgs: I) {
        for m in msgs {
            self.push(m);
        }
    }

    pub fn as_slice(&self) -> &[String] {
        &self.items
    }

    pub fn to_vec(&self) -> Vec<String> {
        self.items.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_and_caps() {
        let mut d = DiagnosticsLog::default();
        d.push("a");
        d.push("a");
        d.push("");
        assert_eq!(d.as_slice(), ["a"]);
        d.extend((0..100).map(|i| format!("m{i}")));
        assert_eq!(d.as_slice().len(), MAX_DIAGNOSTICS);
        assert_eq!(d.as_slice()[0], "a");
    }
}
