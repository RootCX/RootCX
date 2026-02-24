pub fn shell_command() -> (&'static str, &'static str) {
    if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") }
}
