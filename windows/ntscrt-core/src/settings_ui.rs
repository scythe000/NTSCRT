//! ntsc-rs's own settings schema, re-exported for building the NTSC panel.
//!
//! The macOS app generates that panel from a JSON descriptor tree produced by
//! `Vendor/ntscrs-capi` (`ntsc_settings_descriptors_json`) and parsed back in
//! `Sources/CrtApp/NtscSettingsModel.swift`. Rust can skip both halves: the
//! descriptors are already typed values, so the UI walks them directly and
//! reads and writes fields through the accessor pair each descriptor carries.
//!
//! Sixty-odd controls come from this tree, so it stays generated rather than
//! hand-written — which also means the panel picks up new ntsc-rs settings
//! when the submodule is updated, exactly as the Mac build does.

pub use ntsc_rs::settings::standard::NtscEffectFullSettings;
pub use ntsc_rs::settings::{
    AnySetting, GetSetFieldError, MenuItem, SettingDescriptor, SettingID, SettingKind, Settings,
    SettingsList,
};

use crate::ntsc::NtscStage;

/// Descriptor tree for the full ntsc-rs settings set, in the order ntsc-rs
/// declares them.
pub fn descriptors() -> Vec<SettingDescriptor<NtscEffectFullSettings>> {
    SettingsList::<NtscEffectFullSettings>::new().setting_descriptors.into_vec()
}

impl NtscStage {
    /// Read one setting by its descriptor id.
    pub fn get_any(&self, id: &SettingID<NtscEffectFullSettings>) -> AnySetting {
        (id.get)(self.settings())
    }

    /// Write one setting by its descriptor id.
    ///
    /// Returns the same error ntsc-rs would: a type mismatch means the caller
    /// passed a variant the field cannot hold.
    pub fn set_any(
        &mut self,
        id: &SettingID<NtscEffectFullSettings>,
        value: AnySetting,
    ) -> Result<(), GetSetFieldError> {
        (id.set)(self.settings_mut(), value)?;
        // Settings changed, so any cached clean frame is still valid (it is
        // the *input*, not the output) — nothing to invalidate here.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(descs: &[SettingDescriptor<NtscEffectFullSettings>]) -> usize {
        descs
            .iter()
            .map(|d| match &d.kind {
                SettingKind::Group { children } => 1 + count(children),
                _ => 1,
            })
            .sum()
    }

    #[test]
    fn the_schema_is_large_and_well_formed() {
        let descs = descriptors();
        assert!(!descs.is_empty());
        // The README describes "about sixty more" settings beyond the headline
        // ones; guard against the tree collapsing to a stub.
        assert!(count(&descs) > 40, "only {} settings found", count(&descs));
        for d in &descs {
            assert!(!d.label.is_empty());
            assert!(!d.id.name.is_empty());
        }
    }

    #[test]
    fn settings_round_trip_through_the_accessors() {
        let descs = descriptors();
        let mut stage = NtscStage::new();

        // Find a boolean leaf and flip it.
        fn find_bool(
            descs: &[SettingDescriptor<NtscEffectFullSettings>],
        ) -> Option<SettingID<NtscEffectFullSettings>> {
            for d in descs {
                match &d.kind {
                    SettingKind::Boolean => return Some(d.id.clone()),
                    SettingKind::Group { children } => {
                        if let Some(f) = find_bool(children) {
                            return Some(f);
                        }
                    }
                    _ => {}
                }
            }
            None
        }

        let id = find_bool(&descs).expect("schema should contain a boolean");
        let AnySetting::Bool(before) = stage.get_any(&id) else {
            panic!("boolean descriptor did not read back as a bool");
        };
        stage.set_any(&id, AnySetting::Bool(!before)).unwrap();
        assert_eq!(stage.get_any(&id), AnySetting::Bool(!before));
    }

    #[test]
    fn a_type_mismatch_is_reported_not_silently_ignored() {
        let descs = descriptors();
        let mut stage = NtscStage::new();
        let bool_id = descs
            .iter()
            .find_map(|d| matches!(d.kind, SettingKind::Boolean).then(|| d.id.clone()))
            .or_else(|| {
                descs.iter().find_map(|d| match &d.kind {
                    SettingKind::Group { children } => children
                        .iter()
                        .find_map(|c| matches!(c.kind, SettingKind::Boolean).then(|| c.id.clone())),
                    _ => None,
                })
            })
            .expect("schema should contain a boolean");

        assert!(stage.set_any(&bool_id, AnySetting::Float(1.5)).is_err());
    }
}
