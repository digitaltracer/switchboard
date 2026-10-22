//! The relay: a small public service that phones and hubs both dial out to. It forwards each
//! phone's end-to-end encrypted request to that phone's hub by relay id, and sends pushes with
//! the app publisher's APNs key. It runs no agents and stores nothing.

pub mod proto;
pub mod seal;
pub mod server;

#[cfg(test)]
mod tests;
