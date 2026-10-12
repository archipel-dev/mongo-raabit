# Query builder interaction reference

The current sidebar in PR #21 is a prototype, not Studio 3T interaction parity. Moving a
field catalogue into a right island does not reproduce the collection-first workflow.
Keep the Archipel/JetBrains Kit theme and existing icons; match the data interactions.

## Reference evidence

Inspected on 2026-10-12: official Desktop IDE documentation, collection screenshot and
published drag-and-drop animation. Studio 3T is not installed on the test Mac, so this
is not a claim of testing its native executable.

- [Desktop Visual Query Builder](https://studio3t.com/knowledge-base/desktop-ide/visual-query-builder/)
- [Collection tab and editable main query bar](https://studio3t.com/knowledge-base/3t-build/desktop-ide/collections-tab/)
- [Official collection/builder screenshot](https://studio3t.com/wp-content/uploads/2018/11/drag-and-drop-query-builder-1024x622.png)
- [Official result-field drag animation](https://studio3t.com/wp-content/uploads/2024/02/visual-query-builder-drag-and-drop.gif)

Use these Desktop references. The newer Web IDE has different controls and is not the
reference requested here. Link reference artwork; do not copy it into the product.

## Gaps and acceptance criteria

| Interaction | Desktop reference | Current Raabit | Required acceptance |
|---|---|---|---|
| Entry into the builder | Results cell drag, toolbar button, result context menu, Cmd/Ctrl+B | Sidebar is open by default; schema field list is the drag source | Begin a drag on a result cell with the sidebar hidden; reveal the sidebar during the drag without losing the payload. Also support the button, context menu and shortcut. |
| Drag source | Actual cells, not column headers; Table selects one field, Tree supports modifier multi-selection | FieldDrag carries path and schema FieldType only | Carry result field path and actual BSON value/type. Keep header resize/reorder separate. Read-only results can build queries. |
| Main layout | Query, Projection and Sort directly beside results; compact two-line conditions | A seven-row Fields catalogue precedes Filter; large three-line condition cards | Make the three query sections the primary sidebar content. Put schema suggestions in field controls/add-row actions rather than consuming the top of the sidebar. Keep all actions reachable at 1024x700. |
| Manual field entry | Double-click the add/drop row to choose a field | A separate Condition button and editable field TextField | Double-click the add row opens field suggestions; keyboard entry is equally usable. |
| Grouping | Match all/any/none; nested AND/OR groups, including array element matching | All/Any groups; nested group has no field drop zone; no None/elemMatch builder | Drop into the intended nested group, not the root; preserve parentheses and group semantics. Add None and array-group support with BSON query tests. |
| Rule actions | Type selector, duplicate and delete, advanced/multiline value editors | Delete and generic type selector; no duplicate or advanced editor | Clone a rule with operator/value/type intact; edit long strings, regex and special BSON values without losing information. |
| Query representation | Editable Query, Projection, Sort, Skip and Limit bar above results mirrors the builder | A read-only find summary; code entry requires switching the sidebar to Text | Keep the bar editable and synchronized with the visual model. Editing a supported query in either view updates the other. Preserve unsupported expressions instead of dropping them. |
| Execution | Run in collection toolbar and builder; results remain beside the builder | Run only in the sidebar | Both Run actions execute the same active collection query. No execution merely because a condition changes; show when results belong to the previous query. |
| Types | Data type detected from result fields; explicit BSON type controls | Generic Number combines numeric types; payload has no actual value | Preserve Int32/Int64/Double/Decimal128 and special BSON values through cell -> rule -> text -> BSON. Test empty string separately from missing value and null. |

The screenshot also shows per-rule/section enable controls and drag handles. Their exact
native move/disable edge cases still need direct verification; do not claim them tested
from a still image.

## Execution correctness beyond visual similarity

The current core deliberately omits invalid/incomplete conditions from generated text.
`query_builder::show` can still emit Run (including Cmd/Ctrl+Enter). A half-built filter
can therefore execute a broader query than the visible rows imply. Before shipping the
new interaction, invalid active rules must block execution with a clear explanation.
Only explicitly disabled rules may be omitted. This is a Raabit correctness requirement,
not an unverified claim about Studio 3T's validation behavior.

## Kit/SDK boundaries

- Kit owns table/tree cell hit targets, selection, drag threshold, keyboard/context-menu
  access and clipping. Raabit must not paint a private table or duplicate Kit controls.
- Result payloads belong to Raabit: collection/tab identity, dotted field path and BSON
  value. Reject a stale drop after switching collections; never consume another payload
  type or translate a header-reorder payload into a filter.
- Opening the Workbench island during a drag must preserve the source identity/payload
  and the destination under the pointer. Empty nested groups need their own drop targets.
- SDK owns standalone command routing and modal Settings. The query bar, rule model and
  MongoDB execution belong to Raabit/core.

## Required end-to-end scenarios

1. On `people`, drag the Berlin cell to Query with the right island initially hidden.
   The island opens during drag; the rule names `city`, preserves String/`Berlin`, and
   the editable query bar reflects it. Run returns only Ada and Grace.
2. Drag `age` into a nested Any group, add a second typed condition, duplicate it, change
   one operator and run. Verify the exact BSON and result set, not just UI state.
3. Drag `name` to Projection and `age` to Sort. Toggle include/exclude and ascending/
   descending, reorder sort keys, and verify the actual returned fields/order.
4. Edit the same query in the main bar, then the visual rules. Both preserve each other;
   an unsupported expression remains intact with an explicit limitation.
5. Switch collection tabs during drag and while editing a rule. Never apply a payload
   or partially typed value to the other collection. Hide/reopen the island without
   resetting that collection's draft.
6. Try mouse, keyboard and context-menu paths, Escape, rapid drag/release, nested targets,
   horizontal/vertical scrolling and resizing in both themes at 1280x800 and 1024x700.

PR #21 remains draft despite its green baseline CI. Its existing tests prove the prototype
works as designed; they do not prove the reference interactions above.
