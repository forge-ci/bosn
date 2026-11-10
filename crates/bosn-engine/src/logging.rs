/// Prefix daemon stderr records with Unix milliseconds so a restart and an in-flight
/// request can be correlated from the rotated `daemon.log` files (#662).
pub fn daemon_log(message: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for line in message.lines() {
        eprintln!("{stamp} {line}");
    }
}
