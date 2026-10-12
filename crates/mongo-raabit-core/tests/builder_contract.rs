//! The visual query builder is deterministic: the same builder state always gives
//! byte-identical query text, that text parses, and reading it back into the builder gives
//! the same text again (builder → text → builder → text is a fixpoint). Random builder
//! states cover every operator, value type, nesting, sort, projection and paging.

use mongo_raabit_core::builder::{
    Builder, Condition, FieldType, Group, Item, MatchMode, Operator, Projection, ProjectionMode,
    SortKey, TypeName, ValueType,
};
use proptest::prelude::*;

const FIELDS: [&str; 9] = [
    "name",
    "age",
    "_id",
    "address.city",
    "ownerId",
    "tags",
    "créé",
    "with space",
    "a.b.c",
];

const VALUES: [&str; 16] = [
    "",
    "Ada",
    "42",
    "-1.5",
    "1e3",
    "true",
    "false",
    "2024-01-02",
    "2024-01-02T03:04:05Z",
    "5f1d7b2c9a1e4b3c2d1e0f9a",
    "a, b, 3",
    "quote\"s and 'single'",
    "back\\slash",
    "ünï cødé ☃",
    "x.*+?(y)",
    "  padded  ",
];

fn field() -> impl Strategy<Value = String> {
    prop::sample::select(&FIELDS[..]).prop_map(str::to_owned)
}

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(&VALUES[..]).prop_map(str::to_owned),
        "[a-zA-Z0-9 ,.$\"'\\\\/:-]{0,12}",
    ]
}

fn condition() -> impl Strategy<Value = Condition> {
    (
        field(),
        prop::sample::select(&Operator::ALL[..]),
        text(),
        text(),
        prop::sample::select(&ValueType::ALL[..]),
        prop::sample::select(&TypeName::ALL[..]),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(field, operator, value, value2, value_type, type_name, ignore_case, awaiting)| {
                Condition {
                    field,
                    operator,
                    value,
                    value2,
                    value_type,
                    type_name,
                    ignore_case,
                    awaiting_value: awaiting,
                }
            },
        )
}

fn mode() -> impl Strategy<Value = MatchMode> {
    prop::sample::select(&MatchMode::ALL[..])
}

fn group() -> impl Strategy<Value = Group> {
    let leaf = (mode(), prop::collection::vec(condition(), 0..4)).prop_map(|(mode, items)| Group {
        mode,
        items: items.into_iter().map(Item::Condition).collect(),
    });
    leaf.prop_recursive(2, 12, 4, |inner| {
        (
            mode(),
            prop::collection::vec(
                prop_oneof![
                    3 => condition().prop_map(Item::Condition),
                    1 => inner.prop_map(Item::Group),
                ],
                0..4,
            ),
        )
            .prop_map(|(mode, items)| Group { mode, items })
    })
}

fn builder() -> impl Strategy<Value = Builder> {
    (
        group(),
        prop::collection::vec(
            (field(), any::<bool>()).prop_map(|(field, descending)| SortKey { field, descending }),
            0..4,
        ),
        (
            prop::sample::select(&[ProjectionMode::Include, ProjectionMode::Exclude][..]),
            prop::collection::vec(field(), 0..4),
            any::<bool>(),
        )
            .prop_map(|(mode, fields, exclude_id)| Projection {
                mode,
                fields,
                exclude_id,
            }),
        prop_oneof![Just(0u64), 1u64..10_000],
        prop_oneof![Just(0u64), 1u64..10_000],
    )
        .prop_map(|(filter, sort, projection, skip, limit)| Builder {
            filter,
            sort,
            projection,
            skip,
            limit,
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        ..ProptestConfig::default()
    })]

    #[test]
    fn the_same_builder_state_gives_the_same_query_text(b in builder()) {
        let text = b.to_query_text();
        prop_assert_eq!(&text, &b.clone().to_query_text());
        prop_assert_eq!(&text, &b.to_query_text());
        let parsed = text.parse();
        prop_assert!(parsed.is_ok(), "{:?} does not parse: {:?}", text, parsed.err());
    }

    #[test]
    fn builder_text_reads_back_to_the_same_text(b in builder()) {
        let text = b.to_query_text();
        let back = Builder::from_query_text(&text);
        prop_assert!(back.is_ok(), "{:?} not taken over: {:?}", text, back.err());
        let back = back.unwrap();
        prop_assert_eq!(&back.to_query_text(), &text);
        // And once more: the read-back state is stable too.
        let again = Builder::from_query_text(&back.to_query_text()).unwrap();
        prop_assert_eq!(again, back);
    }

    #[test]
    fn dropping_fields_keeps_the_text_deterministic(
        b in builder(),
        drops in prop::collection::vec(
            (field(), prop::sample::select(&FieldType::ALL[..]), 0u8..3),
            0..6,
        ),
    ) {
        let mut one = b.clone();
        let mut two = b;
        for (field, kind, zone) in drops {
            for builder in [&mut one, &mut two] {
                match zone {
                    0 => {
                        builder.add_filter_field(&field, kind);
                    }
                    1 => {
                        builder.add_projection_field(&field);
                    }
                    _ => {
                        builder.add_sort_field(&field);
                    }
                }
            }
        }
        prop_assert_eq!(&one, &two);
        let text = one.to_query_text();
        prop_assert_eq!(&text, &two.to_query_text());
        let back = Builder::from_query_text(&text);
        prop_assert!(back.is_ok(), "{:?} not taken over: {:?}", text, back.err());
        prop_assert_eq!(back.unwrap().to_query_text(), text);
    }
}
