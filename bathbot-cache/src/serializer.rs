use rkyv::{
    ser::serializers::{
        AlignedSerializer, AllocScratch, CompositeSerializer, FallbackScratch, HeapScratch,
    },
    AlignedVec, Infallible,
};

pub(crate) type FullSerializer<const N: usize> = CompositeSerializer<
    AlignedSerializer<AlignedVec>,
    FallbackScratch<HeapScratch<N>, AllocScratch>,
    Infallible,
>;
