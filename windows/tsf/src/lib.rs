#![cfg_attr(not(windows), allow(dead_code))]
#[cfg(windows)]
#[path = "../../candidate_draw.rs"]
mod candidate_draw;
#[cfg(windows)]
#[path = "../../plain_cell.rs"]
mod plain_cell;
#[cfg(windows)]
#[path = "../../process_security.rs"]
mod process_security;
#[cfg(windows)]
mod tsf;
