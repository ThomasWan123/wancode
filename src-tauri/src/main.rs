// Prevents the extra console window on Windows (debug builds included —
// we routinely hand debug builds to the user).
#![windows_subsystem = "windows"]

fn main() {
    wancode_lib::run()
}
