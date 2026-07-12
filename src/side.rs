use serde::{Deserialize, Serialize};

/// Which logical side a mod belongs to — client-only, server-only, both, or `None` for a project
/// a provider marks as neither required nor supported anywhere. `None` is kept distinct from
/// `Both` so the distinction survives a round-trip instead of being silently widened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Client,
    Server,
    #[default]
    Both,
    /// Neither client nor server (a Modrinth project unsupported on both environments).
    None,
}

impl Side {
    /// The token packwiz writes in a `.pw.toml` `side` field. packwiz omits the field entirely
    /// for `Both`, and uses an empty string for the neither-side case.
    pub fn packwiz_token(self) -> &'static str {
        match self {
            Side::Client => "client",
            Side::Server => "server",
            Side::Both => "both",
            Side::None => "",
        }
    }

    /// Whether packwiz would serialize this side at all (`Both` is the implicit default).
    pub fn is_default(self) -> bool {
        matches!(self, Side::Both)
    }

    /// Derive the side from a provider's per-environment support flags
    /// (Modrinth's `client_side` / `server_side`). Neither-supported yields `None`.
    pub fn from_env(client_supported: bool, server_supported: bool) -> Side {
        match (client_supported, server_supported) {
            (true, true) => Side::Both,
            (true, false) => Side::Client,
            (false, true) => Side::Server,
            (false, false) => Side::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_maps_support_flags_to_the_four_sides() {
        assert_eq!(Side::from_env(true, true), Side::Both);
        assert_eq!(Side::from_env(true, false), Side::Client);
        assert_eq!(Side::from_env(false, true), Side::Server);
        assert_eq!(Side::from_env(false, false), Side::None);
    }

    #[test]
    fn packwiz_token_maps_each_side_to_its_field_value() {
        assert_eq!(Side::Client.packwiz_token(), "client");
        assert_eq!(Side::Server.packwiz_token(), "server");
        assert_eq!(Side::Both.packwiz_token(), "both");
        assert_eq!(Side::None.packwiz_token(), "");
    }

    #[test]
    fn packwiz_token_for_none_is_empty_and_collides_with_no_real_side() {
        let none = Side::None.packwiz_token();

        assert!(none.is_empty());
        for other in [Side::Client, Side::Server, Side::Both] {
            assert_ne!(other.packwiz_token(), none);
        }
    }

    #[test]
    fn is_default_is_true_only_for_both() {
        assert!(Side::Both.is_default());
        assert!(!Side::Client.is_default());
        assert!(!Side::Server.is_default());
        assert!(!Side::None.is_default());
    }

    #[test]
    fn default_side_is_both_and_reports_itself_as_default() {
        assert_eq!(Side::default(), Side::Both);
        assert!(Side::default().is_default());
    }

    #[test]
    fn serde_renames_each_variant_to_its_lowercase_token() {
        assert_eq!(serde_json::to_string(&Side::Client).unwrap(), "\"client\"");
        assert_eq!(serde_json::to_string(&Side::Server).unwrap(), "\"server\"");
        assert_eq!(serde_json::to_string(&Side::Both).unwrap(), "\"both\"");
        assert_eq!(serde_json::to_string(&Side::None).unwrap(), "\"none\"");
    }

    #[test]
    fn serde_none_uses_the_word_not_the_empty_packwiz_token() {
        let json = serde_json::to_string(&Side::None).unwrap();

        assert_eq!(json, "\"none\"");
        assert_ne!(json.trim_matches('"'), Side::None.packwiz_token());
    }

    #[test]
    fn every_variant_round_trips_through_json() {
        for side in [Side::Client, Side::Server, Side::Both, Side::None] {
            let json = serde_json::to_string(&side).unwrap();
            let back: Side = serde_json::from_str(&json).unwrap();

            assert_eq!(back, side);
        }
    }

    #[test]
    fn none_survives_a_round_trip_without_widening_to_both() {
        let json = serde_json::to_string(&Side::None).unwrap();

        let back: Side = serde_json::from_str(&json).unwrap();

        assert_eq!(back, Side::None);
        assert_ne!(back, Side::Both);
    }

    #[test]
    fn deserializes_each_lowercase_token_back_to_its_variant() {
        assert_eq!(
            serde_json::from_str::<Side>("\"client\"").unwrap(),
            Side::Client
        );
        assert_eq!(
            serde_json::from_str::<Side>("\"server\"").unwrap(),
            Side::Server
        );
        assert_eq!(
            serde_json::from_str::<Side>("\"both\"").unwrap(),
            Side::Both
        );
        assert_eq!(
            serde_json::from_str::<Side>("\"none\"").unwrap(),
            Side::None
        );
    }

    #[test]
    fn rejects_unknown_miscased_and_empty_tokens() {
        for bad in [
            "\"neither\"",
            "\"CLIENT\"",
            "\"Both\"",
            "\"\"",
            "\"client \"",
        ] {
            assert!(
                serde_json::from_str::<Side>(bad).is_err(),
                "should reject {bad}"
            );
        }
    }

    #[test]
    fn the_four_sides_are_pairwise_distinct() {
        let all = [Side::Client, Side::Server, Side::Both, Side::None];

        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(i == j, a == b, "eq mismatch for {a:?} vs {b:?}");
            }
        }
    }
}
