//! Visual stage forms of the Aggregation Editor: headless rendering of every form and,
//! when a local mongod is reachable, pipelines built only from forms.

use mongo_raabit::MongoRaabit;
use mongo_raabit::app::SessionState;
use mongo_raabit::core::aggregate::{Pipeline, Stage};
use mongo_raabit::core::bson::{Document, doc};
use mongo_raabit::core::profile::ConnectionProfile;
use mongo_raabit::core::stage_form::{
    Accumulator, AccumulatorRow, AddFieldRow, AddFieldsForm, Combine, Condition, GroupForm,
    LookupForm, MatchForm, MatchOp, ProjectAction, ProjectForm, ProjectRow, SortForm, SortRow,
    StageForm, UnwindForm, ValueSource,
};
use mongo_raabit::stage_builder::{self, Suggestions};
use std::time::Duration;

fn uri() -> String {
    std::env::var("MONGO_RAABIT_TEST_URI").unwrap_or_else(|_| "mongodb://127.0.0.1:27017".into())
}

fn stage(operator: &str, form: &StageForm) -> Stage {
    Stage {
        operator: operator.into(),
        body: form.to_body().unwrap(),
        enabled: true,
    }
}

fn sample_stages() -> Vec<Stage> {
    vec![
        stage(
            "$match",
            &StageForm::Match(MatchForm {
                combine: Combine::Any,
                conditions: vec![
                    Condition::new("amount", MatchOp::Gte, "100"),
                    Condition::new("region", MatchOp::Contains, "nor"),
                    Condition::new("flag", MatchOp::Exists, "true"),
                ],
            }),
        ),
        stage(
            "$group",
            &StageForm::Group(GroupForm {
                keys: vec!["region".into()],
                accumulators: vec![
                    AccumulatorRow::new("total", Accumulator::Sum, "amount"),
                    AccumulatorRow::new("n", Accumulator::Count, ""),
                ],
            }),
        ),
        stage(
            "$sort",
            &StageForm::Sort(SortForm {
                fields: vec![SortRow {
                    field: "total".into(),
                    descending: true,
                }],
            }),
        ),
        stage(
            "$project",
            &StageForm::Project(ProjectForm {
                exclude_id: true,
                fields: vec![ProjectRow {
                    field: "_id".into(),
                    action: ProjectAction::RenameTo("region".into()),
                }],
            }),
        ),
        stage("$limit", &StageForm::Limit(5)),
        stage("$skip", &StageForm::Skip(1)),
        stage("$count", &StageForm::Count("groups".into())),
        stage(
            "$unwind",
            &StageForm::Unwind(UnwindForm {
                path: "items".into(),
                preserve_empty: true,
            }),
        ),
        stage(
            "$lookup",
            &StageForm::Lookup(LookupForm {
                from: "regions".into(),
                local_field: "region".into(),
                foreign_field: "_id".into(),
                as_field: "info".into(),
            }),
        ),
        stage(
            "$addFields",
            &StageForm::AddFields(AddFieldsForm {
                fields: vec![
                    AddFieldRow::new("copy", ValueSource::Field, "amount"),
                    AddFieldRow::new("source", ValueSource::Value, "form"),
                ],
            }),
        ),
        stage(
            "$set",
            &StageForm::AddFields(AddFieldsForm {
                fields: vec![AddFieldRow::new("n", ValueSource::Value, "1")],
            }),
        ),
    ]
}

#[test]
fn renders_every_stage_form_headlessly() {
    let fields = vec!["region".to_owned(), "amount".to_owned()];
    let collections = vec!["regions".to_owned()];
    for dark in [true, false] {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        for mut stage in sample_stages() {
            let original = stage.body.clone();
            for _ in 0..2 {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let suggest = Suggestions {
                            fields: &fields,
                            collections: &collections,
                        };
                        let id = ui.make_persistent_id(("form", &stage.operator));
                        assert!(stage_builder::form_available(ui, id, &stage));
                        let shown = stage_builder::stage_form(ui, &mut stage, suggest, id);
                        assert_eq!(shown, Some(false), "{}", stage.operator);
                    });
                });
            }
            assert_eq!(
                stage.body, original,
                "rendering alone never rewrites the body"
            );
        }
        // Bodies the form cannot show stay in code.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut stage = Stage {
                    operator: "$match".into(),
                    body: "{$expr: {$gt: ['$a', '$b']}}".into(),
                    enabled: true,
                };
                let id = ui.make_persistent_id("code-only");
                assert!(!stage_builder::form_available(ui, id, &stage));
                assert_eq!(
                    stage_builder::stage_form(ui, &mut stage, Suggestions::default(), id),
                    None
                );
            });
        });
    }
}

#[test]
fn renders_the_aggregation_tab_with_forms_and_code() {
    let mut app = MongoRaabit::new(None);
    let mut stages = sample_stages();
    stages.push(Stage::new("$facet"));
    let count = stages.len();
    let tab = app.open_aggregation(0, "db", "sales", Some(Pipeline { stages }));
    app.aggregation_tab_mut(tab).unwrap().stage_code.insert(1);
    let ctx = egui::Context::default();
    for selected in 0..count {
        app.aggregation_tab_mut(tab).unwrap().selected = selected;
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.ui(ui));
            });
        }
    }
    assert_eq!(
        app.aggregation_tab_mut(tab).unwrap().pipeline.stages.len(),
        count
    );
}

#[test]
fn field_paths_come_from_input_documents() {
    let docs = vec![
        doc! {"_id": 1, "a": {"b": 1, "c": {"d": {"e": 1}}}},
        doc! {"_id": 2, "z": true},
    ];
    assert_eq!(
        stage_builder::field_paths(&docs),
        ["_id", "a", "a.b", "a.c", "a.c.d", "z"]
    );
}

#[test]
fn form_built_pipelines_run_on_a_live_server() {
    let (mut profile, _) = ConnectionProfile::from_uri(&uri()).unwrap();
    profile.connect_timeout_ms = Some(1500);
    let mut app = MongoRaabit::new(None);
    let session = app.connect(profile);
    app.wait_idle(Duration::from_secs(10));
    if let SessionState::Failed(err) = &app.session(session).unwrap().state {
        eprintln!(
            "skipping form_built_pipelines_run_on_a_live_server: no MongoDB server at {} ({err})",
            uri()
        );
        return;
    }
    let conn = app.session(session).unwrap().connection().unwrap().clone();
    let db = "raabit_test_stage_forms";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let _ = conn.drop_database(db).await;
        conn.insert_documents(
            db,
            "sales",
            (0..30)
                .map(|i| {
                    doc! {
                        "region": (["north", "south", "east"][i % 3]),
                        "amount": i as i32 * 10,
                        "items": (0..(i % 3) as i32).collect::<Vec<_>>(),
                    }
                })
                .collect(),
        )
        .await
        .unwrap();
        conn.insert_documents(
            db,
            "regions",
            vec![
                doc! {"_id": "north", "manager": "Ada"},
                doc! {"_id": "east", "manager": "Grace"},
            ],
        )
        .await
        .unwrap();
    });

    let run = |app: &mut MongoRaabit, stages: Vec<Stage>| -> Vec<Document> {
        let tab = app.open_aggregation(session, db, "sales", Some(Pipeline { stages }));
        app.run_pipeline(tab);
        app.wait_idle(Duration::from_secs(10));
        let tab = app.aggregation_tab_mut(tab).unwrap();
        assert_eq!(tab.stage_error, None);
        tab.result
            .ready()
            .expect("pipeline result")
            .documents
            .clone()
    };

    // Filter, group, sort: the same result as the hand-written pipeline.
    let docs = run(
        &mut app,
        vec![
            stage(
                "$match",
                &StageForm::Match(MatchForm {
                    combine: Combine::All,
                    conditions: vec![Condition::new("amount", MatchOp::Gte, "100")],
                }),
            ),
            stage(
                "$group",
                &StageForm::Group(GroupForm {
                    keys: vec!["region".into()],
                    accumulators: vec![
                        AccumulatorRow::new("total", Accumulator::Sum, "amount"),
                        AccumulatorRow::new("n", Accumulator::Count, ""),
                    ],
                }),
            ),
            stage(
                "$sort",
                &StageForm::Sort(SortForm {
                    fields: vec![SortRow {
                        field: "_id".into(),
                        descending: false,
                    }],
                }),
            ),
        ],
    );
    assert_eq!(
        docs,
        vec![
            doc! {"_id": "east", "total": 1400, "n": 7},
            doc! {"_id": "north", "total": 1170, "n": 6},
            doc! {"_id": "south", "total": 1330, "n": 7},
        ]
    );

    // OR conditions with contains and in, then a count.
    let docs = run(
        &mut app,
        vec![
            stage(
                "$match",
                &StageForm::Match(MatchForm {
                    combine: Combine::Any,
                    conditions: vec![
                        Condition::new("region", MatchOp::Contains, "NOR"),
                        Condition::new("amount", MatchOp::In, "10, 20"),
                    ],
                }),
            ),
            stage("$count", &StageForm::Count("hits".into())),
        ],
    );
    assert_eq!(docs, vec![doc! {"hits": 12}]);

    // Unwind, join, project with rename, add fields, skip and limit.
    let docs = run(
        &mut app,
        vec![
            stage(
                "$match",
                &StageForm::Match(MatchForm {
                    combine: Combine::All,
                    conditions: vec![Condition::new("amount", MatchOp::Lt, "60")],
                }),
            ),
            stage(
                "$unwind",
                &StageForm::Unwind(UnwindForm {
                    path: "items".into(),
                    preserve_empty: false,
                }),
            ),
            stage(
                "$lookup",
                &StageForm::Lookup(LookupForm {
                    from: "regions".into(),
                    local_field: "region".into(),
                    foreign_field: "_id".into(),
                    as_field: "info".into(),
                }),
            ),
            stage(
                "$addFields",
                &StageForm::AddFields(AddFieldsForm {
                    fields: vec![
                        AddFieldRow::new("value", ValueSource::Field, "amount"),
                        AddFieldRow::new("source", ValueSource::Value, "form"),
                    ],
                }),
            ),
            stage(
                "$project",
                &StageForm::Project(ProjectForm {
                    exclude_id: true,
                    fields: vec![
                        ProjectRow {
                            field: "value".into(),
                            action: ProjectAction::Include,
                        },
                        ProjectRow {
                            field: "items".into(),
                            action: ProjectAction::Include,
                        },
                        ProjectRow {
                            field: "source".into(),
                            action: ProjectAction::Include,
                        },
                        ProjectRow {
                            field: "info.manager".into(),
                            action: ProjectAction::RenameTo("manager".into()),
                        },
                    ],
                }),
            ),
            stage(
                "$sort",
                &StageForm::Sort(SortForm {
                    fields: vec![
                        SortRow {
                            field: "value".into(),
                            descending: false,
                        },
                        SortRow {
                            field: "items".into(),
                            descending: false,
                        },
                    ],
                }),
            ),
            stage("$skip", &StageForm::Skip(1)),
            stage("$limit", &StageForm::Limit(2)),
        ],
    );
    // amounts < 60 with items: 10 (1 item), 20 (2 items), 40 (1), 50 (2).
    assert_eq!(
        docs,
        vec![
            doc! {"items": 0, "value": 20, "source": "form", "manager": ["Grace"]},
            doc! {"items": 1, "value": 20, "source": "form", "manager": ["Grace"]},
        ]
    );
    rt.block_on(async {
        conn.drop_database(db).await.unwrap();
    });
}
