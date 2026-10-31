//! Przygotowanie (raz, przy starcie) pakietów fazy configuration z oficjalnych danych.

use mc_data::{GameData, SYNCHRONIZED_REGISTRIES};
use mc_protocol::packets::{self, KnownPack, RegistryEntry};

pub const BRAND: &str = "vanilla";
pub const DATA_VERSION_NAME: &str = "1.21.1";

pub struct ConfigPackets {
    pub brand: Vec<u8>,
    pub features: Vec<u8>,
    pub select_known_packs: Vec<u8>,
    pub known_packs: Vec<KnownPack>,
    /// Wpisy bez danych (klient użyje własnych z paczki `minecraft:core`).
    pub registry_known: Vec<Vec<u8>>,
    /// Wpisy z pełnymi danymi NBT (gdy klient nie ma identycznej paczki).
    pub registry_full: Vec<Vec<u8>>,
    pub tags: Vec<u8>,
    pub finish: Vec<u8>,
}

impl ConfigPackets {
    pub fn build(data: &GameData) -> Self {
        let known_packs = vec![KnownPack { namespace: "minecraft".into(), id: "core".into(), version: DATA_VERSION_NAME.into() }];
        let mut registry_known = Vec::new();
        let mut registry_full = Vec::new();
        for (name, _) in SYNCHRONIZED_REGISTRIES {
            let reg = data.dynamic_registry(name).expect("registry loaded");
            let known: Vec<RegistryEntry> = reg.entries.iter().map(|e| RegistryEntry { id: e.id.clone(), data: None }).collect();
            let full: Vec<RegistryEntry> = reg
                .entries
                .iter()
                .map(|e| RegistryEntry { id: e.id.clone(), data: Some(mc_data::nbtjson::json_to_nbt(&reg.network_json(e))) })
                .collect();
            registry_known.push(packets::config_registry_data(name, &known));
            registry_full.push(packets::config_registry_data(name, &full));
        }
        Self {
            brand: packets::config_brand(BRAND),
            features: packets::config_update_enabled_features(&["minecraft:vanilla"]),
            select_known_packs: packets::config_select_known_packs(&known_packs),
            known_packs,
            registry_known,
            registry_full,
            tags: packets::config_update_tags(&data.tags),
            finish: packets::config_finish(),
        }
    }
}
