//! BSON values as the kit colors them.

use archipel_sdk::kit::ValueKind;
use mongo_raabit_core::bson::Bson;

/// The kit value kind of a BSON value: it picks the color in tables, trees and code views.
pub fn kind(value: &Bson) -> ValueKind {
    match value {
        Bson::String(_) | Bson::Symbol(_) => ValueKind::String,
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => {
            ValueKind::Number
        }
        Bson::Boolean(_) => ValueKind::Boolean,
        Bson::Null | Bson::Undefined | Bson::MinKey | Bson::MaxKey => ValueKind::Null,
        Bson::ObjectId(_) => ValueKind::Id,
        Bson::Binary(_) => ValueKind::Binary,
        Bson::DateTime(_) | Bson::Timestamp(_) => ValueKind::Date,
        Bson::Document(_) | Bson::Array(_) => ValueKind::Punctuation,
        _ => ValueKind::String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongo_raabit_core::bson::{DateTime, doc, oid::ObjectId};

    #[test]
    fn bson_types_map_to_value_kinds() {
        assert_eq!(kind(&Bson::String("a".into())), ValueKind::String);
        assert_eq!(kind(&Bson::Int32(1)), ValueKind::Number);
        assert_eq!(kind(&Bson::Double(1.5)), ValueKind::Number);
        assert_eq!(kind(&Bson::Boolean(true)), ValueKind::Boolean);
        assert_eq!(kind(&Bson::Null), ValueKind::Null);
        assert_eq!(kind(&Bson::ObjectId(ObjectId::new())), ValueKind::Id);
        assert_eq!(kind(&Bson::DateTime(DateTime::now())), ValueKind::Date);
        assert_eq!(kind(&Bson::Document(doc! {"a": 1})), ValueKind::Punctuation);
        assert_eq!(kind(&Bson::Array(vec![])), ValueKind::Punctuation);
    }
}
