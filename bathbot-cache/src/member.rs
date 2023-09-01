use std::time::Duration;

use pin_project::pin_project;
use redlight::{
    config::{Cacheable, ICachedMember},
    error::BoxedError,
    rkyv_util::{id::IdRkyvMap, util::ImageHashRkyv},
    CachedArchive,
};
use rkyv::{option::ArchivedOption, with::Map, Archive, Deserialize, Infallible, Serialize};
use twilight_model::{
    gateway::payload::incoming::MemberUpdate,
    guild::{Member, PartialMember},
    id::{
        marker::{GuildMarker, RoleMarker},
        Id,
    },
    util::ImageHash,
};

use crate::serializer::FullSerializer;

#[derive(Archive, Deserialize, Serialize)]
#[archive(check_bytes)]
#[archive_attr(pin_project)]
pub struct CachedMember {
    #[with(Map<ImageHashRkyv>)]
    pub avatar: Option<ImageHash>,
    pub nick: Option<String>,
    #[with(IdRkyvMap)]
    pub roles: Vec<Id<RoleMarker>>,
}

impl CachedMember {
    fn update(
        archive: &mut CachedArchive<Self>,
        avatar: Option<ImageHash>,
        nick: Option<&str>,
        roles: &[Id<RoleMarker>],
    ) -> Result<(), BoxedError> {
        if archive.nick.as_deref() == nick && archive.roles.iter().eq(roles.iter()) {
            archive.update_archive(|pinned| {
                let this = pinned.project();

                *this.avatar = avatar
                    .map(ImageHash::into)
                    .map_or(ArchivedOption::None, ArchivedOption::Some);
            });

            Ok(())
        } else {
            archive.update_by_deserializing(
                |deserialized| {
                    deserialized.avatar = avatar;
                    deserialized.nick = nick.map(str::to_owned);
                    deserialized.roles = roles.to_owned();
                },
                &mut Infallible,
            )
        }
    }
}

impl<'a> ICachedMember<'a> for CachedMember {
    fn from_member(_: Id<GuildMarker>, member: &'a Member) -> Self {
        Self {
            avatar: member.avatar,
            nick: member.nick.clone(),
            roles: member.roles.clone(),
        }
    }

    fn update_via_partial(
    ) -> Option<fn(&mut CachedArchive<Self>, &PartialMember) -> Result<(), BoxedError>> {
        Some(|archive, partial| {
            CachedMember::update(
                archive,
                partial.avatar,
                partial.nick.as_deref(),
                &partial.roles,
            )
        })
    }

    fn on_member_update(
    ) -> Option<fn(&mut CachedArchive<Self>, &MemberUpdate) -> Result<(), BoxedError>> {
        Some(|archive, update| {
            CachedMember::update(
                archive,
                update.avatar,
                update.nick.as_deref(),
                &update.roles,
            )
        })
    }
}

impl Cacheable for CachedMember {
    // TODO: test
    type Serializer = FullSerializer<128>;

    fn expire() -> Option<Duration> {
        None
    }
}
