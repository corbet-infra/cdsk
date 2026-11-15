//! cdsk's integration tests: every scenario runs against recorded fixtures and
//! plans actions; nothing touches a pool or a filesystem.

mod common;
mod disks;
mod layout;
mod owner;
mod save;
mod shape;
mod zfs;
