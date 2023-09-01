use std::time::Duration;

use pin_project::pin_project;
use redlight::{
    config::{Cacheable, ICachedGuild},
    error::BoxedError,
    rkyv_util::{
        id::IdRkyv,
        util::{BitflagsRkyv, ImageHashRkyv},
    },
    CachedArchive,
};
use rkyv::{
    option::ArchivedOption, ser::serializers::BufferSerializer, with::Map, AlignedBytes, Archive,
    Deserialize, Infallible, Serialize,
};
use twilight_model::{
    gateway::payload::incoming::GuildUpdate,
    guild::{Guild, Permissions},
    id::{
        marker::{GuildMarker, UserMarker},
        Id,
    },
    util::ImageHash,
};

#[derive(Archive, Deserialize, Serialize)]
#[archive(check_bytes)]
#[archive_attr(pin_project)]
pub struct CachedGuild {
    #[with(Map<ImageHashRkyv>)]
    pub icon: Option<ImageHash>,
    #[with(IdRkyv)]
    pub id: Id<GuildMarker>,
    pub name: Box<str>,
    #[with(IdRkyv)]
    pub owner_id: Id<UserMarker>,
    #[with(Map<BitflagsRkyv>)]
    pub permissions: Option<Permissions>,
}

impl CachedGuild {
    fn update(archive: &mut CachedArchive<Self>, update: &GuildUpdate) -> Result<(), BoxedError> {
        if archive.name.as_ref() == update.name {
            archive.update_archive(|pinned| {
                let this = pinned.project();

                *this.icon = update
                    .icon
                    .map(ImageHash::into)
                    .map_or(ArchivedOption::None, ArchivedOption::Some);

                *this.id = update.id.into();
                *this.owner_id = update.owner_id.into();

                *this.permissions = update
                    .permissions
                    .as_ref()
                    .map(Permissions::bits)
                    .map_or(ArchivedOption::None, ArchivedOption::Some);
            });

            Ok(())
        } else {
            archive.update_by_deserializing(
                |deserialized| {
                    deserialized.icon = update.icon;
                    deserialized.id = update.id;
                    deserialized.name = update.name.as_str().into();
                    deserialized.owner_id = update.owner_id;
                    deserialized.permissions = update.permissions;
                },
                &mut Infallible,
            )
        }
    }
}

impl<'a> ICachedGuild<'a> for CachedGuild {
    fn from_guild(guild: &'a Guild) -> Self {
        Self {
            icon: guild.icon,
            id: guild.id,
            name: guild.name.as_str().into(),
            owner_id: guild.owner_id,
            permissions: guild.permissions,
        }
    }

    fn on_guild_update(
    ) -> Option<fn(&mut CachedArchive<Self>, &GuildUpdate) -> Result<(), BoxedError>> {
        Some(Self::update)
    }
}

impl Cacheable for CachedGuild {
    // 168 bytes are sufficient for names of length 100
    // which is the upper limit for guild name length
    type Serializer = BufferSerializer<AlignedBytes<168>>;

    fn expire() -> Option<Duration> {
        None
    }
}
