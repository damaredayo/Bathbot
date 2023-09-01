mod config;
mod serializer;

pub mod channel;
pub mod current_user;
pub mod guild;
pub mod member;
pub mod role;
pub mod user;

pub use redlight;

pub type Cache = redlight::RedisCache<config::Config>;
