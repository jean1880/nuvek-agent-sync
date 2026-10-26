//! The one output boundary. Everything agent-sync prints (findings, content diffs, permission
//! lists, parser errors, a CLI's stderr inside an error chain) goes through [`scrub`], so a
//! secret that reached a projection, a source file or an error message is never echoed.

/// Redact secret-shaped tokens and `key = value` credentials.
pub fn scrub(text: &str) -> String {
    nuvek_core::scrub::redact_config(text)
}

/// The audit logger, scrubbed: every message is redacted before it reaches the journal or
/// stderr. The only logger agent-sync uses.
pub struct Log(nuvek_core::log::Logger);

impl Log {
    /// The journal (or stderr, with `NUVEK_LOG_SINK=stderr`) under `name`.
    pub fn get(name: &str) -> Log {
        Log(nuvek_core::log::get_logger(name))
    }
    /// stderr only, for tests.
    #[cfg(test)]
    pub fn stderr_only(name: &str) -> Log {
        Log(nuvek_core::log::Logger::stderr_only(name))
    }
    pub fn info(&self, msg: &str) {
        self.0.info(&scrub(msg));
    }
    pub fn warn(&self, msg: &str) {
        self.0.warn(&scrub(msg));
    }
    pub fn error(&self, msg: &str) {
        self.0.error(&scrub(msg));
    }
}

/// `println!`, scrubbed.
macro_rules! say {
    () => {
        println!()
    };
    ($($t:tt)*) => {
        println!("{}", $crate::out::scrub(&format!($($t)*)))
    };
}

/// `eprintln!`, scrubbed.
macro_rules! say_err {
    () => {
        eprintln!()
    };
    ($($t:tt)*) => {
        eprintln!("{}", $crate::out::scrub(&format!($($t)*)))
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn credentials_are_redacted_at_the_boundary() {
        let token = format!("ghp_{}", "Zq9Xw8Vu7Ts6Rq5Po4Nm3Lk2Jh1Gf0Ed9Cb8A");
        let pw = format!("pass{}=hunter2-very-secret", "word");
        let out = super::scrub(&format!("+ {token}\n- {pw}"));
        assert!(!out.contains(&token), "{out}");
        assert!(!out.contains("hunter2-very-secret"), "{out}");
    }
}
