use std::time::Duration;

use redlight::{
    config::{Cacheable, ICachedCurrentUser},
    rkyv_util::{id::IdRkyv, util::ImageHashRkyv},
};
use rkyv::{
    ser::serializers::BufferSerializer,
    with::{Map, RefAsBox},
    AlignedBytes, Archive, Serialize,
};
use twilight_model::{
    id::{marker::UserMarker, Id},
    user::CurrentUser,
    util::ImageHash,
};

#[derive(Archive, Serialize)]
#[archive(check_bytes)]
pub struct CachedCurrentUser<'a> {
    #[with(Map<ImageHashRkyv>)]
    pub avatar: Option<ImageHash>,
    pub discriminator: u16,
    #[with(IdRkyv)]
    pub id: Id<UserMarker>,
    #[with(RefAsBox)]
    pub name: &'a str,
}

impl<'a> ICachedCurrentUser<'a> for CachedCurrentUser<'a> {
    fn from_current_user(current_user: &'a CurrentUser) -> Self {
        Self {
            avatar: current_user.avatar,
            discriminator: current_user.discriminator,
            id: current_user.id,
            name: &current_user.name,
        }
    }
}

impl Cacheable for CachedCurrentUser<'_> {
    // 56 bytes are sufficient for the name "Bathbot-Dev"
    type Serializer = BufferSerializer<AlignedBytes<56>>;

    fn expire() -> Option<Duration> {
        None
    }
}
