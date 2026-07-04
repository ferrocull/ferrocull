const MAX_HISTORY: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct JobCodeHistory {
    codes: Vec<String>,
}

impl JobCodeHistory {
    /// Builds a history from persisted codes, ordered most-recent first.
    #[must_use]
    pub fn from_codes(codes: Vec<String>) -> Self {
        Self { codes }
    }

    /// Add a job code to history. Moves it to front if already present.
    /// Trims to [`MAX_HISTORY`] items.
    pub fn add(&mut self, code: &str) {
        self.codes.retain(|c| c != code);
        self.codes.insert(0, code.to_owned());
        self.codes.truncate(MAX_HISTORY);
    }

    #[must_use]
    pub fn codes(&self) -> &[String] {
        &self.codes
    }
}
