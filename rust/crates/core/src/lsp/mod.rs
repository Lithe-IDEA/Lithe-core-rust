//! Language tooling is split by responsibility while this facade keeps the Core command API stable.

pub(crate) mod interface;
mod languages;

pub(crate) use interface::*;
pub(crate) use lithe_text::*;

#[cfg(test)]
mod tests;
