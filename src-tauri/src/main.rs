// Prevents an additional console window on Windows GUI launches.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    desktop_lib::run()
}
