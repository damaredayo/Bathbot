use std::time::Duration;

use redlight::{
    config::{Cacheable, ICachedRole},
    rkyv_util::{id::IdRkyv, util::BitflagsRkyv},
};
use rkyv::{ser::serializers::AlignedSerializer, with::RefAsBox, AlignedVec, Archive, Serialize};
use twilight_model::{
    guild::{Permissions, Role},
    id::{marker::RoleMarker, Id},
};

#[derive(Archive, Serialize)]
#[archive(check_bytes)]
pub struct CachedRole<'a> {
    #[with(IdRkyv)]
    pub id: Id<RoleMarker>,
    #[with(RefAsBox)]
    pub name: &'a str,
    #[with(BitflagsRkyv)]
    pub permissions: Permissions,
    pub position: i64,
}

impl<'a> ICachedRole<'a> for CachedRole<'a> {
    fn from_role(role: &'a Role) -> Self {
        Self {
            id: role.id,
            name: &role.name,
            permissions: role.permissions,
            position: role.position,
        }
    }
}

impl Cacheable for CachedRole<'_> {
    // Role names have no apparent character limit
    type Serializer = AlignedSerializer<AlignedVec>;

    fn expire() -> Option<Duration> {
        None
    }
}
