//! The asset store: registered asset types, the assets loaded by identifier, and the
//! read-only `Assets` system parameter.

use std::any::{Any, TypeId, type_name};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;

use ludenea_schema::TypeSchema;
use xxhash_rust::xxh3::Xxh3;

use crate::component::decode_exact;
use crate::query::Access;
use crate::system::{ASSET, Accesses, Missing, SystemParam, declare_cell};
use crate::world::check_same_type;
use crate::{Asset, AssetId, Codec, EcsError, Handle, World, hash_value};

/// A decoded asset; `Sync` so systems may share it across threads, and shared so a
/// command may hold it while it changes the world.
pub(crate) type AssetValue = Arc<dyn Any + Send + Sync>;

/// One registered asset type. `decode` and `decode_ron` mirror the codec's, which give
/// values that are only `Send` (components need not be `Sync`), as shared `Sync` values.
pub(crate) struct AssetType {
    type_id: TypeId,
    type_name: &'static str,
    codec: Codec,
    decode: fn(&[u8]) -> Result<AssetValue, String>,
    decode_ron: fn(&str) -> Result<AssetValue, String>,
    default: Option<fn() -> AssetValue>,
}

/// One loaded asset: its type's stable name, its value, and its canonical postcard bytes.
pub(crate) struct LoadedAsset {
    pub(crate) type_name: &'static str,
    pub(crate) value: AssetValue,
    pub(crate) bytes: Vec<u8>,
}

impl World {
    /// Registers the asset type `T`; registering the same type again is a no-op.
    pub fn register_asset<T: Asset>(&mut self) -> Result<(), EcsError> {
        if let Some(entry) = self.asset_types.get(T::NAME) {
            return check_same_type::<T>(T::NAME, entry.type_id, entry.type_name);
        }
        self.asset_types.insert(
            T::NAME,
            AssetType {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                codec: Codec::of::<T>(),
                decode: |bytes| Ok(Arc::new(decode_exact::<T>(bytes)?)),
                decode_ron: |text| {
                    let value: T = ron::from_str(text).map_err(|error| error.to_string())?;
                    Ok(Arc::new(value))
                },
                default: None,
            },
        );
        Ok(())
    }

    /// Registers the asset type `T` and records `T::default` as the value of a new asset.
    pub fn register_asset_with_default<T: Asset + Default>(&mut self) -> Result<(), EcsError> {
        self.register_asset::<T>()?;
        self.asset_types
            .get_mut(T::NAME)
            .expect("just registered")
            .default = Some(|| Arc::new(T::default()));
        Ok(())
    }

    /// The schema of every registered asset type, by stable name.
    pub fn asset_schemas(&self) -> Vec<(&'static str, TypeSchema)> {
        self.asset_types
            .iter()
            .map(|(&name, entry)| (name, (entry.codec.schema)()))
            .collect()
    }

    /// The codec of the asset type `type_name`.
    pub fn asset_codec(&self, type_name: &str) -> Option<Codec> {
        self.asset_types.get(type_name).map(|entry| entry.codec)
    }

    /// The default value of the asset type `type_name` as a RON string.
    pub fn default_asset_ron(&self, type_name: &str) -> Result<String, EcsError> {
        let entry = self.asset_type(type_name)?;
        let default = entry
            .default
            .ok_or_else(|| EcsError::NoDefault(type_name.to_string()))?;
        Ok((entry.codec.encode_ron)(&*default()))
    }

    /// Inserts or replaces the asset `id` from postcard `bytes` of the type `type_name`.
    pub fn insert_asset(
        &mut self,
        id: AssetId,
        type_name: &str,
        bytes: &[u8],
    ) -> Result<(), EcsError> {
        let decode = self.asset_type(type_name)?.decode;
        self.store_asset(id, type_name, decode(bytes))
    }

    /// Inserts or replaces the asset `id` from a RON text of the type `type_name`. On a
    /// parse error the reason starts with the line and column.
    pub fn insert_asset_ron(
        &mut self,
        id: AssetId,
        type_name: &str,
        ron: &str,
    ) -> Result<(), EcsError> {
        let decode_ron = self.asset_type(type_name)?.decode_ron;
        self.store_asset(id, type_name, decode_ron(ron))
    }

    /// Loaded asset identifiers in ascending order.
    pub fn asset_ids(&self) -> Vec<AssetId> {
        self.assets.keys().copied().collect()
    }

    /// The type name and canonical postcard bytes of the loaded asset `id`.
    pub fn asset_bytes(&self, id: AssetId) -> Option<(&str, Vec<u8>)> {
        let asset = self.assets.get(&id)?;
        Some((asset.type_name, asset.bytes.clone()))
    }

    /// The loaded asset `id` as a single-line RON string.
    pub fn asset_ron(&self, id: AssetId) -> Option<String> {
        let asset = self.assets.get(&id)?;
        let codec = self.asset_types[asset.type_name].codec;
        Some((codec.encode_ron)(&*asset.value))
    }

    /// Unloads every asset.
    pub fn clear_assets(&mut self) {
        self.assets.clear();
    }

    /// xxh3 over each loaded asset, in identifier order: its identifier as 16 little-endian
    /// bytes, then the `hash_value` of its canonical bytes.
    pub(crate) fn assets_hash(&self) -> u64 {
        let mut hasher = Xxh3::new();
        for (id, asset) in &self.assets {
            hasher.update(&id.0.to_le_bytes());
            hasher.update(&hash_value(&asset.bytes[..]).to_le_bytes());
        }
        hasher.digest()
    }

    /// Decodes `bytes` of `type_name` into a loaded asset, without storing it.
    pub(crate) fn decode_asset(
        &self,
        id: AssetId,
        type_name: &str,
        bytes: &[u8],
    ) -> Result<(AssetId, LoadedAsset), EcsError> {
        let decode = self.asset_type(type_name)?.decode;
        self.loaded(id, type_name, decode(bytes))
    }

    /// Replaces every loaded asset.
    pub(crate) fn replace_assets(&mut self, assets: Vec<(AssetId, LoadedAsset)>) {
        self.assets = assets.into_iter().collect();
    }

    fn store_asset(
        &mut self,
        id: AssetId,
        type_name: &str,
        decoded: Result<AssetValue, String>,
    ) -> Result<(), EcsError> {
        let (id, asset) = self.loaded(id, type_name, decoded)?;
        self.assets.insert(id, asset);
        Ok(())
    }

    fn loaded(
        &self,
        id: AssetId,
        type_name: &str,
        decoded: Result<AssetValue, String>,
    ) -> Result<(AssetId, LoadedAsset), EcsError> {
        let (&name, entry) = self
            .asset_types
            .get_key_value(type_name)
            .ok_or_else(|| EcsError::UnknownAsset(type_name.to_string()))?;
        let value = decoded.map_err(|reason| EcsError::InvalidAsset {
            id,
            type_name: type_name.to_string(),
            reason,
        })?;
        let bytes = (entry.codec.encode)(&*value);
        Ok((
            id,
            LoadedAsset {
                type_name: name,
                value,
                bytes,
            },
        ))
    }

    fn asset_type(&self, type_name: &str) -> Result<&AssetType, EcsError> {
        self.asset_types
            .get(type_name)
            .ok_or_else(|| EcsError::UnknownAsset(type_name.to_string()))
    }
}

/// Reads loaded assets of type `T` by handle. Assets never change during a tick, so this
/// parameter never conflicts with another.
pub struct Assets<'w, T: Asset> {
    assets: &'w BTreeMap<AssetId, LoadedAsset>,
    asset: PhantomData<fn() -> T>,
}

impl<'w, T: Asset> Assets<'w, T> {
    /// # Panics
    /// If the asset of `handle` is not loaded (or is not a `T`).
    pub fn get(&self, handle: Handle<T>) -> &'w T {
        self.try_get(handle)
            .unwrap_or_else(|| panic!("asset {} of type {} is not loaded", handle.id, T::NAME))
    }

    /// The asset of `handle`, or `None` when it is not loaded (or is not a `T`).
    pub fn try_get(&self, handle: Handle<T>) -> Option<&'w T> {
        self.assets.get(&handle.id)?.value.downcast_ref()
    }
}

impl<T: Asset> SystemParam for Assets<'_, T> {
    type Item<'w> = Assets<'w, T>;

    fn declare(world: &World, accesses: &mut Accesses) -> Result<(), Missing> {
        declare_cell(
            world.asset_types.get(T::NAME),
            ASSET,
            Access::Read(T::NAME),
            accesses,
        )
    }

    fn fetch(world: &World) -> Self::Item<'_> {
        Assets {
            assets: &world.assets,
            asset: PhantomData,
        }
    }
}
