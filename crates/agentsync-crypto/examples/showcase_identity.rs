//! Pipe an ephemeral demonstration identity to the showcase supervisor.
//! Never invoke this helper in a logging pipeline or persist its output.
use age::secrecy::ExposeSecret;
use std::io::{self, Write};

fn main() -> io::Result<()> {
    let identity = age::x25519::Identity::generate();
    let mut output = io::stdout().lock();
    writeln!(output, "{}", identity.to_public())?;
    writeln!(output, "{}", identity.to_string().expose_secret())
}
