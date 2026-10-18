use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextForm<'a> {
    Delta(&'a str),
    Cumulative(&'a str),
}

#[derive(Default)]
pub struct TextReconciler {
    units: HashMap<String, TextUnit>,
}

#[derive(Default)]
struct TextUnit {
    observed: String,
    emitted: usize,
}

impl TextReconciler {
    /// Reconcile one text unit in amortized O(new bytes). Cumulative snapshots
    /// may extend or repeat the current text. A shorter prefix is an out-of-order
    /// stale observation and emits nothing; only genuinely divergent text fails.
    pub fn observe(&mut self, unit_id: &str, form: TextForm<'_>) -> Result<String, &'static str> {
        let unit = self.units.entry(unit_id.to_owned()).or_default();
        match form {
            TextForm::Delta(delta) => unit.observed.push_str(delta),
            TextForm::Cumulative(snapshot) => {
                if snapshot.starts_with(&unit.observed) {
                    unit.observed.clear();
                    unit.observed.push_str(snapshot);
                } else if unit.observed.starts_with(snapshot) {
                    return Ok(String::new());
                } else {
                    return Err("native_text_snapshot_diverged");
                }
            }
        }
        if !unit.observed.is_char_boundary(unit.emitted) {
            return Err("native_text_boundary_invalid");
        }
        let suffix = unit.observed[unit.emitted..].to_owned();
        unit.emitted = unit.observed.len();
        Ok(suffix)
    }

    pub fn observed(&self, unit_id: &str) -> Option<&str> {
        self.units.get(unit_id).map(|unit| unit.observed.as_str())
    }
}
