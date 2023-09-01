use std::time::Duration;

use pin_project::pin_project;
use redlight::{
    config::{Cacheable, ICachedUser},
    error::BoxedError,
    rkyv_util::{id::IdRkyv, util::ImageHashRkyv},
    CachedArchive,
};
use rkyv::{
    option::ArchivedOption,
    ser::serializers::BufferSerializer,
    with::{Map, RefAsBox},
    AlignedBytes, Archive, Serialize,
};
use twilight_model::{
    gateway::payload::incoming::invite_create::PartialUser,
    id::{marker::UserMarker, Id},
    user::User,
    util::ImageHash,
};

#[derive(Archive, Serialize)]
#[archive(check_bytes)]
#[archive_attr(pin_project)]
pub struct CachedUser<'a> {
    #[with(Map<ImageHashRkyv>)]
    pub avatar: Option<ImageHash>,
    pub bot: bool,
    pub discriminator: u16,
    #[with(IdRkyv)]
    pub id: Id<UserMarker>,
    #[with(RefAsBox)]
    pub name: &'a str,
}

impl<'a> ICachedUser<'a> for CachedUser<'a> {
    fn from_user(user: &'a User) -> Self {
        Self {
            avatar: user.avatar,
            bot: user.bot,
            discriminator: user.discriminator,
            id: user.id,
            name: &user.name,
        }
    }

    fn update_via_partial(
    ) -> Option<fn(&mut CachedArchive<Self>, &PartialUser) -> Result<(), BoxedError>> {
        Some(|archived, partial| {
            archived.update_archive(|pinned| {
                let this = pinned.project();

                *this.avatar = partial
                    .avatar
                    .map(ImageHash::into)
                    .map_or(ArchivedOption::None, ArchivedOption::Some);

                *this.discriminator = partial.discriminator;
            });

            Ok(())
        })
    }
}

impl Cacheable for CachedUser<'_> {
    // 72 bytes are sufficient for names of length 32
    // which is the upper limit for user name length
    type Serializer = BufferSerializer<AlignedBytes<72>>;

    fn expire() -> Option<Duration> {
        None
    }
}
