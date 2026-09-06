//! The `Module` enum — one tweak in a scenario — plus the M1-subset gate.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// One tweak. Internally tagged by `type`. Any unrecognised `type` deserializes
/// to [`Module::Unsupported`], which [`Module::require_m1_supported`] rejects.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Module {
    Registry(RegistryPayload),
    Powercfg {
        sub: String,
        setting: String,
        value: u32,
    },
    PowerPlan(PowerPlanPayload),
    AffinityCpu(AffinityCpuPayload),
    LaunchArgs {
        args: String,
    },
    Unsupported,
}

/// The `type` tags a JSON document can name. Private: [`Module`]'s own
/// `Deserialize` impl routes any other (or missing) `type` to
/// [`Module::Unsupported`] instead of failing, so a project file authored
/// for a later milestone still loads as a draft and is rejected by
/// [`Module::require_m1_supported`] with a useful message.
///
/// Why not `#[serde(other)]` on `Module::Unsupported`: specta-serde 0.0.12
/// renders an internally-tagged enum that carries `#[serde(other)]` as an
/// externally-wrapped `Module_Serialize`/`Module_Deserialize` pair that does
/// not match serde's real wire format (ARCHITECTURE_PROPOSED.md §1.2 F3).
/// The same enum without `#[serde(other)]` renders correctly, so the
/// tolerance lives here instead.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum KnownModule {
    Registry(RegistryPayload),
    Powercfg {
        sub: String,
        setting: String,
        value: u32,
    },
    PowerPlan(PowerPlanPayload),
    AffinityCpu(AffinityCpuPayload),
    LaunchArgs {
        args: String,
    },
}

const KNOWN_MODULE_TYPES: [&str; 5] = [
    "registry",
    "powercfg",
    "power_plan",
    "affinity_cpu",
    "launch_args",
];

impl<'de> Deserialize<'de> for Module {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let is_known = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|t| KNOWN_MODULE_TYPES.contains(&t));
        if !is_known {
            return Ok(Module::Unsupported);
        }
        let known: KnownModule = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(match known {
            KnownModule::Registry(p) => Module::Registry(p),
            KnownModule::Powercfg {
                sub,
                setting,
                value,
            } => Module::Powercfg {
                sub,
                setting,
                value,
            },
            KnownModule::PowerPlan(p) => Module::PowerPlan(p),
            KnownModule::AffinityCpu(p) => Module::AffinityCpu(p),
            KnownModule::LaunchArgs { args } => Module::LaunchArgs { args },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RegistryPayload {
    pub hive: Hive,
    pub subkey: String,
    pub value_name: String,
    pub value_type: RegType,
    /// The registry value's actual data -- deliberately untyped (its shape
    /// depends on `value_type`: a DWORD needs a number, a SZ needs a
    /// string, etc.). Exported to TypeScript as `unknown`, same reasoning and
    /// same specta-typescript escape hatch as `CatalogEntry.module_template`.
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Unknown))]
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "UPPERCASE")]
pub enum Hive {
    Hklm,
    Hkcu,
}

impl Hive {
    pub fn as_str(&self) -> &'static str {
        match self {
            Hive::Hklm => "HKLM",
            Hive::Hkcu => "HKCU",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "UPPERCASE")]
pub enum RegType {
    Dword,
    Qword,
    Sz,
    Binary,
    /// `REG_EXPAND_SZ`. `#[serde(rename)]` because plain `rename_all =
    /// "UPPERCASE"` would produce `EXPANDSZ`, not the `EXPAND_SZ` the
    /// catalog and `docs/08-security-model.md` §5 use.
    #[serde(rename = "EXPAND_SZ")]
    ExpandSz,
    /// `REG_MULTI_SZ`; see [`ExpandSz`](Self::ExpandSz)'s doc comment for why
    /// this needs an explicit rename too.
    #[serde(rename = "MULTI_SZ")]
    MultiSz,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PowerPlanPayload {
    pub plan_guid: String,
    #[serde(default)]
    pub friendly_name: Option<String>,
    #[serde(default)]
    pub create_if_missing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct AffinityCpuPayload {
    pub mode: AffinityMode,
    #[serde(default)]
    pub mask_hex: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum AffinityMode {
    PcoreOnly,
    Ccd0,
    ExcludeCore0,
    ExplicitMask,
}

/// Defensive upper bound on registry path component length. Matches the
/// `subkey` rule in `docs/08-security-model.md` §5 ("no NUL, length ≤ 512, no
/// `..` segments"); applied to `value_name` too for the same reasons even
/// though the doc table doesn't spell that one out.
const MAX_REGISTRY_COMPONENT_LEN: usize = 512;

/// NUL-byte / length / (`subkey` only) `..`-segment hygiene, per
/// `docs/08-security-model.md` §5. Registry key names have no `..` traversal
/// semantics in the Win32 API, but rejecting the literal segment costs
/// nothing and matches the documented rule exactly.
fn validate_registry_component(s: &str, field: &str, reject_dotdot_segments: bool) -> Result<()> {
    if s.contains('\0') {
        return Err(Error::msg(format!(
            "registry {field} must not contain a NUL byte"
        )));
    }
    if s.len() > MAX_REGISTRY_COMPONENT_LEN {
        return Err(Error::msg(format!(
            "registry {field} exceeds {MAX_REGISTRY_COMPONENT_LEN} characters"
        )));
    }
    if reject_dotdot_segments && s.split('\\').any(|seg| seg == "..") {
        return Err(Error::msg(format!(
            "registry {field} must not contain a `..` segment"
        )));
    }
    Ok(())
}

impl Module {
    pub fn kind(&self) -> &'static str {
        match self {
            Module::Registry(_) => "registry",
            Module::Powercfg { .. } => "powercfg",
            Module::PowerPlan(_) => "power_plan",
            Module::AffinityCpu(_) => "affinity_cpu",
            Module::LaunchArgs { .. } => "launch_args",
            Module::Unsupported => "unsupported",
        }
    }

    /// `Ok` for the five variants M1 can execute. `Err(Error::UnsupportedModule)`
    /// for [`Module::Unsupported`] and for a `Registry` module targeting any key
    /// under `\Enum\` (device-instance keys — these need a SYSTEM token, which
    /// M1 does not have; see [ADR-004](../../../../docs/02-architecture.md)).
    /// `Err(Error::Msg)` for a malformed `subkey` / `value_name` (NUL, overlong,
    /// or a `..` path segment).
    pub fn require_m1_supported(&self) -> Result<()> {
        const REASON: &str =
            "reboot-required / driver / SYSTEM-token modules arrive in milestone M3";
        match self {
            Module::Unsupported => Err(Error::unsupported_module(
                "unsupported".into(),
                REASON.into(),
            )),
            Module::Registry(p) => {
                validate_registry_component(&p.subkey, "subkey", true)?;
                validate_registry_component(&p.value_name, "value_name", false)?;

                // Segment-based match (not a substring `.contains()`): catches
                // any key with an exact `Enum` path segment — including a
                // subkey that ends precisely at `...\Enum\PCI` with no
                // trailing separator, which a substring check on
                // `\enum\pci\` would miss.
                let has_enum_segment = p
                    .subkey
                    .split('\\')
                    .any(|seg| seg.eq_ignore_ascii_case("enum"));
                if has_enum_segment {
                    return Err(Error::unsupported_module(
                        "registry".into(),
                        format!(
                            "keys under \\Enum\\ need a SYSTEM token — milestone M3. ({REASON})"
                        ),
                    ));
                }
                Ok(())
            }
            Module::Powercfg { .. }
            | Module::PowerPlan(_)
            | Module::AffinityCpu(_)
            | Module::LaunchArgs { .. } => Ok(()),
        }
    }

    /// `"{HIVE}\{subkey}\{value_name}"` for a registry module; used by the
    /// conflict check. `None` for every other kind.
    pub fn registry_target(&self) -> Option<String> {
        match self {
            Module::Registry(p) => Some(format!(
                "{}\\{}\\{}",
                p.hive.as_str(),
                p.subkey,
                p.value_name
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(j: &str) -> Module {
        serde_json::from_str(j).unwrap()
    }

    #[test]
    fn parses_supported_registry_module() {
        let m = parse(
            r#"{"type":"registry","hive":"HKLM","subkey":"SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers","value_name":"HwSchMode","value_type":"DWORD","value":2}"#,
        );
        assert_eq!(m.kind(), "registry");
        m.require_m1_supported().unwrap();
    }

    #[test]
    fn unknown_type_becomes_unsupported_and_is_rejected() {
        let m = parse(r#"{"type":"driver_install","package":"x.exe"}"#);
        assert_eq!(m.kind(), "unsupported");
        let e = m.require_m1_supported().unwrap_err();
        assert!(e.to_string().to_lowercase().contains("m3"));
    }

    #[test]
    fn enum_pci_registry_path_is_rejected() {
        let m = parse(
            r#"{"type":"registry","hive":"HKLM","subkey":"SYSTEM\\CurrentControlSet\\Enum\\PCI\\VEN_10DE\\Device Parameters\\Interrupt Management\\Affinity Policy","value_name":"DevicePolicy","value_type":"DWORD","value":4}"#,
        );
        let e = m.require_m1_supported().unwrap_err();
        assert!(e.to_string().contains("M3"));
    }

    /// Regression test: a subkey ending exactly at `...\Enum\PCI`, with no
    /// trailing separator, bypassed the old `.contains(r"\enum\pci\")` check.
    #[test]
    fn enum_segment_without_trailing_separator_is_still_rejected() {
        let m = parse(
            r#"{"type":"registry","hive":"HKLM","subkey":"SYSTEM\\CurrentControlSet\\Enum\\PCI","value_name":"X","value_type":"DWORD","value":1}"#,
        );
        let e = m.require_m1_supported().unwrap_err();
        assert!(e.to_string().contains("M3"));
    }

    /// Any `Enum` segment is rejected, not just PCI (e.g. `\Enum\USB\...`).
    #[test]
    fn non_pci_enum_branch_is_also_rejected() {
        let m = parse(
            r#"{"type":"registry","hive":"HKLM","subkey":"SYSTEM\\CurrentControlSet\\Enum\\USB\\VID_1234","value_name":"X","value_type":"DWORD","value":1}"#,
        );
        assert!(m.require_m1_supported().is_err());
    }

    #[test]
    fn subkey_with_nul_byte_is_rejected() {
        let mut m = parse(
            r#"{"type":"registry","hive":"HKLM","subkey":"SYSTEM\\Control\\X","value_name":"V","value_type":"DWORD","value":1}"#,
        );
        if let Module::Registry(p) = &mut m {
            p.subkey = "SYSTEM\\Control\\X\0Y".into();
        }
        assert!(m.require_m1_supported().is_err());
    }

    #[test]
    fn subkey_with_dotdot_segment_is_rejected() {
        let m = parse(
            r#"{"type":"registry","hive":"HKLM","subkey":"SYSTEM\\Control\\..\\Enum\\PCI","value_name":"V","value_type":"DWORD","value":1}"#,
        );
        assert!(m.require_m1_supported().is_err());
    }

    #[test]
    fn overlong_subkey_is_rejected() {
        let m = parse(&format!(
            r#"{{"type":"registry","hive":"HKLM","subkey":"SYSTEM\\{}","value_name":"V","value_type":"DWORD","value":1}}"#,
            "A".repeat(600)
        ));
        assert!(m.require_m1_supported().is_err());
    }

    #[test]
    fn empty_value_name_is_still_allowed() {
        // The empty string names the registry key's "(Default)" value — it
        // must not be rejected by the value_name validation.
        let m = parse(
            r#"{"type":"registry","hive":"HKCU","subkey":"System\\GameConfigStore","value_name":"","value_type":"SZ","value":"x"}"#,
        );
        m.require_m1_supported().unwrap();
    }

    #[test]
    fn registry_target_is_hive_subkey_value() {
        let m = parse(
            r#"{"type":"registry","hive":"HKCU","subkey":"System\\GameConfigStore","value_name":"GameDVR_FSEBehavior","value_type":"DWORD","value":2}"#,
        );
        assert_eq!(
            m.registry_target().unwrap(),
            r"HKCU\System\GameConfigStore\GameDVR_FSEBehavior"
        );
    }

    #[test]
    fn affinity_mode_parses_exclude_core0() {
        let m = parse(r#"{"type":"affinity_cpu","mode":"exclude_core0"}"#);
        assert!(matches!(
            m,
            Module::AffinityCpu(AffinityCpuPayload {
                mode: AffinityMode::ExcludeCore0,
                ..
            })
        ));
    }

    #[test]
    fn launch_args_with_arbitrary_free_text_passes_unvalidated() {
        // No allowlist/blocklist any more -- see the launch-args rework:
        // "-insecure" used to be blocked, now it's just the user's choice.
        let m = parse(r#"{"type":"launch_args","args":"-insecure -novid whatever"}"#);
        assert_eq!(m.kind(), "launch_args");
        m.require_m1_supported().unwrap();
    }

    #[test]
    fn launch_args_with_empty_string_passes() {
        let m = parse(r#"{"type":"launch_args","args":""}"#);
        m.require_m1_supported().unwrap();
    }

    #[test]
    fn every_known_variant_round_trips_through_json() {
        let modules = vec![
            parse(
                r#"{"type":"registry","hive":"HKLM","subkey":"SYSTEM\\X","value_name":"V","value_type":"DWORD","value":2}"#,
            ),
            parse(r#"{"type":"powercfg","sub":"sub_processor","setting":"IDLEDISABLE","value":1}"#),
            parse(r#"{"type":"power_plan","plan_guid":"8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c"}"#),
            parse(r#"{"type":"affinity_cpu","mode":"exclude_core0"}"#),
            parse(r#"{"type":"launch_args","args":"-novid"}"#),
        ];
        for m in modules {
            let json = serde_json::to_string(&m).unwrap();
            let back: Module = serde_json::from_str(&json).unwrap();
            assert_eq!(back, m, "{json}");
            assert!(
                json.starts_with(r#"{"type":""#),
                "wire format must be internally tagged: {json}"
            );
        }
    }

    #[test]
    fn a_document_without_a_type_field_is_unsupported_not_an_error() {
        let m = parse(r#"{"package":"x.exe"}"#);
        assert_eq!(m.kind(), "unsupported");
    }

    /// Writes `src/lib/contracts/module-wire.fixture.json`: one JSON document
    /// per `Module` variant, exactly as serde emits it. `module-wire.test.ts`
    /// asserts each document is assignable to the generated TypeScript
    /// `Module` type, so the bindings can never drift from the wire again
    /// without a test failing on one side or the other. Set
    /// `VOIDFRAME_WRITE_FIXTURES=1` to rewrite the committed file.
    #[test]
    fn module_wire_fixture_is_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src/lib/contracts/module-wire.fixture.json"
        );
        let modules = vec![
            Module::Registry(RegistryPayload {
                hive: Hive::Hklm,
                subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
                value_name: "HwSchMode".into(),
                value_type: RegType::Dword,
                value: serde_json::json!(2),
            }),
            Module::Powercfg {
                sub: "sub_processor".into(),
                setting: "IDLEDISABLE".into(),
                value: 1,
            },
            Module::PowerPlan(PowerPlanPayload {
                plan_guid: "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".into(),
                friendly_name: Some("High performance".into()),
                create_if_missing: false,
            }),
            Module::AffinityCpu(AffinityCpuPayload {
                mode: AffinityMode::ExcludeCore0,
                mask_hex: None,
            }),
            Module::LaunchArgs {
                args: "-novid -high".into(),
            },
            Module::Unsupported,
        ];
        let fresh = serde_json::to_string_pretty(&modules).unwrap() + "\n";
        if std::env::var_os("VOIDFRAME_WRITE_FIXTURES").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, &fresh).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(path).expect(
            "fixture missing -- run: $env:VOIDFRAME_WRITE_FIXTURES=1; cargo test -p voidframe-engine module_wire_fixture",
        );
        assert_eq!(fresh.replace("\r\n", "\n"), committed.replace("\r\n", "\n"));
    }
}
