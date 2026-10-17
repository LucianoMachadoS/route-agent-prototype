//! GamePrep: a Windows game optimizer. Watches for configured games and,
//! when one starts, closes the background apps you picked (politely
//! first, forcibly only if they won't exit); optionally reopens them when
//! the game ends. Also turns off Game Bar background recording and runs
//! the read-only PC diagnosis from `route_agent::windiag`.
//!
//! Never runs elevated and never needs to: everything it touches belongs
//! to the current user.

pub mod config;
#[cfg(windows)]
pub mod endsession;
pub mod engine;
pub mod icon;
#[cfg(windows)]
pub mod procs;
pub mod protect;
#[cfg(windows)]
pub mod tweaks;
