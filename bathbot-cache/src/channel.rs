use std::time::Duration;

use redlight::{
    config::{Cacheable, ICachedChannel},
    error::BoxedError,
    rkyv_util::{
        id::{IdRkyv, IdRkyvMap},
        util::{BitflagsRkyv, RkyvAsU8},
    },
    CachedArchive,
};
use rkyv::{
    out_field,
    with::{ArchiveWith, DeserializeWith, Map, RefAsBox, SerializeWith, With},
    Archive, Archived, CheckBytes, Deserialize, Fallible, Resolver, Serialize,
};
use twilight_model::{
    channel::{
        permission_overwrite::{PermissionOverwrite, PermissionOverwriteType},
        Channel, ChannelType,
    },
    gateway::payload::incoming::ChannelPinsUpdate,
    guild::Permissions,
    id::{
        marker::{ChannelMarker, GenericMarker, GuildMarker},
        Id,
    },
};

use crate::serializer::FullSerializer;

#[derive(Archive, Deserialize, Serialize)]
#[archive(check_bytes)]
pub struct CachedChannel<'a> {
    #[with(IdRkyvMap)]
    pub guild_id: Option<Id<GuildMarker>>,
    #[with(IdRkyv)]
    pub id: Id<ChannelMarker>,
    #[with(RkyvAsU8)]
    pub kind: ChannelType,
    #[with(Map<RefAsBox>)]
    pub name: Option<&'a str>,
    #[with(IdRkyvMap)]
    pub parent_id: Option<Id<ChannelMarker>>,
    #[with(Map<Map<PermissionOverwriteRkyv>>)]
    pub permission_overwrites: Option<Vec<PermissionOverwrite>>,
    pub position: Option<i32>, // TODO: remove?
}

impl<'a> ICachedChannel<'a> for CachedChannel<'a> {
    fn from_channel(channel: &'a Channel) -> Self {
        Self {
            guild_id: channel.guild_id,
            id: channel.id,
            kind: channel.kind,
            name: channel.name.as_deref(),
            parent_id: channel.parent_id,
            permission_overwrites: channel.permission_overwrites.clone(),
            position: channel.position,
        }
    }

    fn on_pins_update(
    ) -> Option<fn(&mut CachedArchive<Self>, &ChannelPinsUpdate) -> Result<(), BoxedError>> {
        None
    }
}

impl Cacheable for CachedChannel<'_> {
    // TODO: test value
    type Serializer = FullSerializer<128>;

    fn expire() -> Option<Duration> {
        None
    }
}

#[derive(CheckBytes)]
pub struct ArchivedPermissionOverwrite {
    pub allow: Archived<With<Permissions, BitflagsRkyv>>,
    pub deny: Archived<With<Permissions, BitflagsRkyv>>,
    pub id: Archived<With<Id<GenericMarker>, IdRkyv>>,
    pub kind: Archived<With<PermissionOverwriteType, RkyvAsU8>>,
}

pub struct PermissionOverwriteResolver {
    allow: Resolver<With<Permissions, BitflagsRkyv>>,
    deny: Resolver<With<Permissions, BitflagsRkyv>>,
    id: Resolver<With<Id<GenericMarker>, IdRkyv>>,
    kind: Resolver<With<PermissionOverwriteType, RkyvAsU8>>,
}

pub struct PermissionOverwriteRkyv;

impl ArchiveWith<PermissionOverwrite> for PermissionOverwriteRkyv {
    type Archived = ArchivedPermissionOverwrite;
    type Resolver = PermissionOverwriteResolver;

    unsafe fn resolve_with(
        overwrite: &PermissionOverwrite,
        pos: usize,
        resolver: Self::Resolver,
        out: *mut Self::Archived,
    ) {
        let (fp, fo) = out_field!(out.allow);
        BitflagsRkyv::resolve_with(&overwrite.allow, pos + fp, resolver.allow, fo);

        let (fp, fo) = out_field!(out.deny);
        BitflagsRkyv::resolve_with(&overwrite.deny, pos + fp, resolver.deny, fo);

        let (fp, fo) = out_field!(out.id);
        IdRkyv::resolve_with(&overwrite.id, pos + fp, resolver.id, fo);

        let (fp, fo) = out_field!(out.kind);
        RkyvAsU8::resolve_with(&overwrite.kind, pos + fp, resolver.kind, fo);
    }
}

impl<S: Fallible> SerializeWith<PermissionOverwrite, S> for PermissionOverwriteRkyv {
    fn serialize_with(
        overwrite: &PermissionOverwrite,
        serializer: &mut S,
    ) -> Result<Self::Resolver, <S as Fallible>::Error> {
        Ok(PermissionOverwriteResolver {
            allow: BitflagsRkyv::serialize_with(&overwrite.allow, serializer)?,
            deny: BitflagsRkyv::serialize_with(&overwrite.deny, serializer)?,
            id: IdRkyv::serialize_with(&overwrite.id, serializer)?,
            kind: RkyvAsU8::serialize_with(&overwrite.kind, serializer)?,
        })
    }
}

impl<D: Fallible> DeserializeWith<ArchivedPermissionOverwrite, PermissionOverwrite, D>
    for PermissionOverwriteRkyv
{
    fn deserialize_with(
        archived: &ArchivedPermissionOverwrite,
        deserializer: &mut D,
    ) -> Result<PermissionOverwrite, <D as Fallible>::Error> {
        Ok(PermissionOverwrite {
            allow: BitflagsRkyv::deserialize_with(&archived.allow, deserializer)?,
            deny: BitflagsRkyv::deserialize_with(&archived.deny, deserializer)?,
            id: IdRkyv::deserialize_with(&archived.id, deserializer)?,
            kind: RkyvAsU8::deserialize_with(&archived.kind, deserializer)?,
        })
    }
}
