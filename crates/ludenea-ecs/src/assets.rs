//! The asset store: registered asset types, the assets loaded by identifier, and the
//! read-only `Assets` system parameter.

use std::any::{Any, TypeId, type_name};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ludenea_schema::TypeSchema;
use xxhash_rust::xxh3::Xxh3;

use crate::component::decode_exact;
use crate::query::Access;
use crate::system::{ASSET, Accesses, Missing, SystemParam, declare_cell};
use crate::world::{check_not_recursive, check_same_type};
use crate::{Asset, AssetId, Codec, EcsError, Handle, World, hash_value};

/// A decoded asset; `Sync` so systems may share it across threads, and shared so a
/// command may hold it while it changes the world.
pub(crate) type AssetValue = Arc<dyn Any + Send + Sync>;

/// One registered asset type. `decode` and `decode_ron` mirror the codec's, which give
/// values that are only `Send` (components need not be `Sync`), as shared `Sync` values;
/// `validate` is the type's `Asset::validate`.
pub(crate) struct AssetType {
    pub(crate) type_id: TypeId,
    type_name: &'static str,
    codec: Codec,
    decode: fn(&[u8]) -> Result<AssetValue, String>,
    decode_ron: fn(&str) -> Result<AssetValue, String>,
    validate: fn(&World, &AssetValue) -> Result<(), String>,
    default: Option<fn() -> AssetValue>,
}

/// One loaded asset: its type's stable name, its value, and its canonical postcard bytes,
/// both shared so that saving a world clones them cheaply.
#[derive(Clone)]
pub(crate) struct LoadedAsset {
    pub(crate) type_name: &'static str,
    pub(crate) value: AssetValue,
    pub(crate) bytes: Arc<[u8]>,
}

/// The last version given to an asset map; `0` is the empty map of a new world.
static ASSETS_VERSION: AtomicU64 = AtomicU64::new(0);

/// An asset its type decoded and validated, not stored yet: `World::decode_asset` runs the
/// type's own code before `World::insert_decoded_asset` changes the world.
pub struct DecodedAsset {
    pub(crate) id: AssetId,
    pub(crate) asset: LoadedAsset,
}

impl World {
    /// Registers the asset type `T`; registering the same type again is a no-op. Fails with
    /// `EcsError::RecursiveType` when `T` holds a type that contains itself.
    pub fn register_asset<T: Asset>(&mut self) -> Result<(), EcsError> {
        if let Some(entry) = self.asset_types.get(T::NAME) {
            return check_same_type::<T>(T::NAME, entry.type_id, entry.type_name);
        }
        check_not_recursive::<T>()?;
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
                validate: |world, value| {
                    value
                        .downcast_ref::<T>()
                        .expect("decoded by this type")
                        .validate(world)
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
    /// Fails with `EcsError::InvalidAsset`, changing nothing, when the bytes do not decode
    /// exactly or the type's `Asset::validate` refuses the value.
    pub fn insert_asset(
        &mut self,
        id: AssetId,
        type_name: &str,
        bytes: &[u8],
    ) -> Result<(), EcsError> {
        let asset = self.decode_asset(id, type_name, bytes)?;
        self.insert_decoded_asset(asset);
        Ok(())
    }

    /// Decodes and validates `bytes` of `type_name` as the asset `id`, without storing it.
    /// Refuses them as `insert_asset` does.
    pub fn decode_asset(
        &self,
        id: AssetId,
        type_name: &str,
        bytes: &[u8],
    ) -> Result<DecodedAsset, EcsError> {
        let decode = self.asset_type(type_name)?.decode;
        let (id, asset) = self.loaded(id, type_name, decode(bytes))?;
        Ok(DecodedAsset { id, asset })
    }

    /// Stores `asset`, replacing the asset with its identifier. Dropping the value replaced
    /// runs the type's own code.
    pub fn insert_decoded_asset(&mut self, asset: DecodedAsset) {
        self.assets.insert(asset.id, asset.asset);
        self.assets_changed();
    }

    /// Inserts or replaces the asset `id` from a RON text of the type `type_name`, refused
    /// as `insert_asset` refuses bytes. On a parse error the reason starts with the line
    /// and column.
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
        Some((asset.type_name, asset.bytes.to_vec()))
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
        self.assets_changed();
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

    /// Replaces every loaded asset.
    pub(crate) fn replace_assets(&mut self, assets: Vec<DecodedAsset>) {
        self.assets = assets
            .into_iter()
            .map(|decoded| (decoded.id, decoded.asset))
            .collect();
        self.assets_changed();
    }

    /// Gives the asset map a version no asset map of this process had before, so that two
    /// maps of the same version, in any worlds, hold the same assets: a save or a load
    /// copies the map only when the versions differ. Every change to `assets` calls it.
    fn assets_changed(&mut self) {
        // Relaxed: the counter orders nothing else, it only has to give each call a new value.
        self.assets_version = ASSETS_VERSION.fetch_add(1, Ordering::Relaxed) + 1;
    }

    fn store_asset(
        &mut self,
        id: AssetId,
        type_name: &str,
        decoded: Result<AssetValue, String>,
    ) -> Result<(), EcsError> {
        let (id, asset) = self.loaded(id, type_name, decoded)?;
        self.assets.insert(id, asset);
        self.assets_changed();
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
        let value = decoded
            .and_then(|value| (entry.validate)(self, &value).map(|()| value))
            .map_err(|reason| EcsError::InvalidAsset {
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
                bytes: bytes.into(),
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
