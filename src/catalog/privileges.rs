//! [X3.135-1986 §4.15, §6.6] Privileges: what a `<privilege definition>`
//! grants, as the catalog keeps it, and the applicable privileges of an
//! `<authorization identifier>` on a table [X3.135-1986 §6.6 SR3], and
//! the current privileges of an authorization stack's top cell [ISO/IEC
//! 9075-2:2023 §4.42.4].

use std::collections::{BTreeMap, BTreeSet};

use super::{Catalog, QualifiedName, ViewDesc, Who};

/// [X3.135-1986 §6.6] A set of privileges on one table: the `<action>`s,
/// UPDATE per column [X3.135-1986 §6.6 SR2], and INSERT and REFERENCES
/// too [ISO/IEC 9075:1992 §10.3].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Actions {
    /// SELECT.
    pub select: bool,
    /// The positions of the columns INSERT is held on [ISO/IEC 9075:1992
    /// §10.3 GR4]: an INSERT names them alone [§13.8 AR1].
    pub insert: BTreeSet<usize>,
    /// DELETE.
    pub delete: bool,
    /// The positions of the columns UPDATE is held on.
    pub update: BTreeSet<usize>,
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §10.3] The positions of the
    /// columns REFERENCES is held on: a referential constraint may name
    /// them [§11.8 AR1].
    pub references: BTreeSet<usize>,
    /// [ISO/IEC 9075-2:2023 §12.3] TRIGGER (Feature T217): its schema's
    /// owner may define a trigger on the table [§11.49 AR2].
    pub trigger: bool,
    /// Those of a typed table alone.
    pub typed: TypedActions,
}

/// The privileges on a table there are of a typed table alone [ISO/IEC
/// 9075-2:2023 §4.42.2].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypedActions {
    /// [ISO/IEC 9075-2:2023 §12.2] SELECT WITH HIERARCHY OPTION (Feature
    /// S081): SELECT on each of its subtables too \[GR8\], and its rows read
    /// alone under ONLY [§7.6 AR1b ii 2].
    pub hierarchy: bool,
    /// [ISO/IEC 9075-2:2023 §4.42.2, §12.3] The specific names of the
    /// methods of the table's structured type on whose table/method pair
    /// SELECT is held (Feature S024): a method reference of a scope of the
    /// table invokes them [§6.23 AR1].
    pub methods: BTreeSet<QualifiedName>,
}

impl Actions {
    /// Every privilege on a table of `columns` columns.
    pub fn all(columns: usize) -> Actions {
        let every = || (0..columns).collect();
        Actions {
            select: true,
            insert: every(),
            delete: true,
            update: every(),
            references: every(),
            trigger: true,
            typed: TypedActions::default(),
        }
    }

    /// Adds the privileges of `other`.
    pub fn add(&mut self, other: &Actions) {
        self.select |= other.select;
        self.insert.extend(other.insert.iter().copied());
        self.delete |= other.delete;
        self.update.extend(other.update.iter().copied());
        self.references.extend(other.references.iter().copied());
        self.trigger |= other.trigger;
        self.typed.hierarchy |= other.typed.hierarchy;
        self.typed.methods.extend(other.typed.methods.iter().cloned());
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        *self == Actions::default()
    }

    /// Those of these `other` has too.
    #[must_use]
    pub fn common(&self, other: &Actions) -> Actions {
        Actions {
            select: self.select && other.select,
            insert: self.insert.intersection(&other.insert).copied().collect(),
            delete: self.delete && other.delete,
            update: self.update.intersection(&other.update).copied().collect(),
            references: self.references.intersection(&other.references).copied().collect(),
            trigger: self.trigger && other.trigger,
            typed: TypedActions {
                hierarchy: self.typed.hierarchy && other.typed.hierarchy,
                methods: self.typed.methods.intersection(&other.typed.methods).cloned().collect(),
            },
        }
    }

    /// Each privilege, one by one: as SQL-92's privilege descriptors are,
    /// one for each action on the table, or on one of its columns
    /// [ISO/IEC 9075:1992 §10.3 GR3, §11.36 GR3].
    pub fn each(&self) -> Vec<Privilege> {
        let tabled = [
            (self.select, Privilege::Select),
            (self.delete, Privilege::Delete),
            (self.trigger, Privilege::Trigger),
            (self.typed.hierarchy, Privilege::Hierarchy),
        ];
        let mut each: Vec<Privilege> = tabled.into_iter().filter(|(held, _)| *held).map(|(_, p)| p).collect();
        each.extend(self.insert.iter().map(|c| Privilege::Insert(*c)));
        each.extend(self.update.iter().map(|c| Privilege::Update(*c)));
        each.extend(self.references.iter().map(|c| Privilege::References(*c)));
        each.extend(self.typed.methods.iter().map(|m| Privilege::Method(m.clone())));
        each
    }

    /// The privileges `each`, together.
    pub fn of(each: impl IntoIterator<Item = Privilege>) -> Actions {
        let mut actions = Actions::default();
        for p in each {
            match p {
                Privilege::Select => actions.select = true,
                Privilege::Insert(c) => {
                    actions.insert.insert(c);
                }
                Privilege::Delete => actions.delete = true,
                Privilege::Update(c) => {
                    actions.update.insert(c);
                }
                Privilege::References(c) => {
                    actions.references.insert(c);
                }
                Privilege::Trigger => actions.trigger = true,
                Privilege::Hierarchy => actions.typed.hierarchy = true,
                Privilege::Method(m) => {
                    actions.typed.methods.insert(m);
                }
            }
        }
        actions
    }

    /// The first privilege of `other` these lack, named with the column
    /// names `columns` (or positions, where it has none): "SELECT",
    /// "UPDATE(AMT)"; none if they lack none.
    pub fn missing(&self, other: &Actions, columns: &[String]) -> Option<String> {
        let actions = [
            ("SELECT", self.select, other.select),
            ("DELETE", self.delete, other.delete),
            ("TRIGGER", self.trigger, other.trigger),
            ("SELECT WITH HIERARCHY OPTION", self.typed.hierarchy, other.typed.hierarchy),
        ];
        if let Some((name, ..)) = actions.iter().find(|(_, held, wanted)| *wanted && !*held) {
            return Some((*name).to_owned());
        }
        let name = |column: &usize| columns.get(*column).cloned().unwrap_or_else(|| format!("#{column}"));
        if let Some(column) = other.insert.difference(&self.insert).next() {
            return Some(format!("INSERT({})", name(column)));
        }
        if let Some(column) = other.update.difference(&self.update).next() {
            return Some(format!("UPDATE({})", name(column)));
        }
        if let Some(column) = other.references.difference(&self.references).next() {
            return Some(format!("REFERENCES({})", name(column)));
        }
        let method = other.typed.methods.difference(&self.typed.methods).next()?;
        Some(format!("SELECT(SPECIFIC METHOD {method})"))
    }

    /// The column of the privilege `missing` names, if it names one: the
    /// inaccessible column [ISO/IEC 9075:1992 §18.1 GR3g ii].
    pub fn missing_column(&self, other: &Actions) -> Option<usize> {
        if (other.select && !self.select)
            || (other.delete && !self.delete)
            || (other.trigger && !self.trigger)
            || (other.typed.hierarchy && !self.typed.hierarchy)
        {
            return None;
        }
        let mut columns = other.insert.difference(&self.insert);
        let first = columns.next().or_else(|| other.update.difference(&self.update).next());
        first.or_else(|| other.references.difference(&self.references).next()).copied()
    }
}

/// One privilege on a table: an action on it, or on one of its columns
/// [ISO/IEC 9075:1992 §10.3].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Privilege {
    /// SELECT.
    Select,
    /// INSERT on the column at this position.
    Insert(usize),
    /// DELETE.
    Delete,
    /// UPDATE on the column at this position.
    Update(usize),
    /// REFERENCES on the column at this position.
    References(usize),
    /// [ISO/IEC 9075-2:2023 §12.3] TRIGGER.
    Trigger,
    /// [ISO/IEC 9075-2:2023 §12.2] The WITH HIERARCHY OPTION of SELECT.
    Hierarchy,
    /// [ISO/IEC 9075-2:2023 §4.42.2, §12.3] SELECT on the table/method pair
    /// of the table and the method of this specific name.
    Method(QualifiedName),
}

/// [ISO/IEC 9075-2:2023 §11.49 GR6] The special grantor value of the
/// privileges an INSTEAD OF trigger gives the owner of its view's schema.
pub const SYSTEM_GRANTOR: &str = "_SYSTEM";

/// [X3.135-1986 §6.6] What one `<privilege definition>`, or `<grant
/// statement>` [ISO/IEC 9075:1992 §11.36], grants one `<grantee>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegeDesc {
    /// The `<authorization identifier>` that granted it: of the `<schema>`
    /// it is granted in, or the current one of the `<grant statement>`.
    pub grantor: String,
    /// The `<grantee>`: an `<authorization identifier>`, or none for PUBLIC.
    pub grantee: Option<String>,
    /// The table.
    pub table: QualifiedName,
    /// The privileges.
    pub actions: Actions,
    /// WITH GRANT OPTION.
    pub grantable: bool,
}

/// [ISO/IEC 9075:1992 §11.36 GR12, GR13] The privileges `descs` on one
/// table, each once: a privilege granted by one grantor to one grantee
/// both grantable and not is grantable, and those alike are one
/// descriptor, of every privilege a grantor gave a grantee alike, in the
/// order each pair first appears.
pub fn merged(descs: Vec<PrivilegeDesc>) -> Vec<PrivilegeDesc> {
    type Pair = (String, Option<String>);
    let mut pairs: Vec<(Pair, QualifiedName)> = Vec::new();
    let mut each: BTreeMap<(Pair, Privilege), bool> = BTreeMap::new();
    for p in descs {
        let pair = (p.grantor, p.grantee);
        if !pairs.iter().any(|(seen, _)| *seen == pair) {
            pairs.push((pair.clone(), p.table));
        }
        for privilege in p.actions.each() {
            let grantable = each.entry((pair.clone(), privilege)).or_insert(false);
            *grantable |= p.grantable;
        }
    }
    let mut merged = Vec::new();
    for (pair, table) in pairs {
        for grantable in [true, false] {
            let actions = Actions::of(each.iter().filter(|((of, _), g)| *of == pair && **g == grantable).map(|((_, p), _)| p.clone()));
            if !actions.is_empty() {
                let (grantor, grantee) = pair.clone();
                merged.push(PrivilegeDesc { grantor, grantee, table: table.clone(), actions, grantable });
            }
        }
    }
    merged
}

/// [X3.135-1986 §6.6 SR3] The applicable privileges of `who` on the
/// table or view `name`, and those of them that are grantable: of a cell
/// of more than one enabled authorization identifier, the current
/// privileges, those applicable for any of them [ISO/IEC 9075-2:2023
/// §4.42.4].
///
/// The owner, the `<authorization identifier>` that owns its schema
/// (`owner_of`), has every
/// privilege on a base table, all grantable \[SR3b i 1\]; on a read-only
/// view, SELECT, grantable if its SELECT on every table the view names is
/// \[SR3b i 2\]; on an updatable view, those it has on the table the view's
/// `<from clause>` names, column for column \[SR3b i 3\]. Anyone else has
/// those of every `<privilege definition>` on it whose grantees are
/// enabled or PUBLIC \[SR3b ii\], grantable if one of them says WITH GRANT OPTION:
/// SQL-92's applicable privileges are those of PUBLIC too [ISO/IEC
/// 9075:1992 §10.3 GR2], and a grant to PUBLIC permits one by anyone
/// [§11.37 SR7a ii], where SQL-86 took only one naming its grantee itself. On a table of INFORMATION_SCHEMA, everyone has
/// SELECT, grantable, "so that SELECT privilege can be further granted on
/// views that reference" it, and no other privilege [ISO/IEC 9075:1992
/// §21.1]; on one of SQL/MM Spatial's ST_INFORMTN_SCHEMA, the same
/// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §19.1], and on one of Simple
/// feature access's SF_INFORMTN_SCHEMA (RULES.md D136).
pub fn applicable(catalog: &dyn Catalog, who: Who<'_>, name: &QualifiedName) -> (Actions, Actions) {
    let information = [crate::system::INFORMATION_SCHEMA, crate::system::ST_INFORMTN_SCHEMA, crate::system::SF_INFORMTN_SCHEMA];
    if information.contains(&name.auth.as_str()) {
        let select = Actions { select: true, ..Actions::default() };
        return (select.clone(), select);
    }
    let owner = super::owner_of(catalog, &name.auth);
    if !who.includes(owner) {
        return granted(catalog, who, name);
    }
    if let Some(table) = catalog.table(name) {
        // [ISO/IEC 9075-2:2023 §11.3 GR7] of a typed table, SELECT WITH
        // HIERARCHY OPTION, and SELECT on the table/method pair of each
        // method of its type.
        let methods = table.typed.as_ref().map(|t| super::type_methods(catalog, &t.udt)).unwrap_or_default();
        let all = Actions { typed: TypedActions { hierarchy: table.typed.is_some(), methods }, ..Actions::all(table.columns.len()) };
        return (all.clone(), all);
    }
    let Some(view) = catalog.view(name) else { return (Actions::default(), Actions::default()) };
    let sole = Who::sole(who.roles, owner);
    let (mut held, mut grantable) = derived(catalog, sole, view);
    // [ISO/IEC 9075-2:2023 §11.32 GR5] TRIGGER on its view, not grantable.
    (held.trigger, grantable.trigger) = (true, false);
    // [ISO/IEC 9075-2:2023 §11.32 GR8a] SELECT WITH HIERARCHY OPTION on a
    // referenceable view, grantable; \[DESIGN\] and, as of a typed table
    // [§11.3 GR7], SELECT on the table/method pair of each method of its
    // type, grantable, so a method is invoked of a reference scoped by it
    // (RULES.md D81).
    if let Some(typed) = &view.typed {
        (held.typed.hierarchy, grantable.typed.hierarchy) = (true, true);
        let methods = super::type_methods(catalog, &typed.udt);
        held.typed.methods.extend(methods.iter().cloned());
        grantable.typed.methods.extend(methods);
    }
    let (mut held, mut grantable) = system_granted(catalog, sole, name, (held, grantable));
    // [ISO/IEC 9075-2:2023 §4.42.4] And those granted another enabled one.
    if who.auth != owner || who.role.is_some() {
        let (more, more_grantable) = granted(catalog, who, name);
        held.add(&more);
        grantable.add(&more_grantable);
    }
    (held, grantable)
}

/// [ISO/IEC 9075-2:2023 §11.49 GR6] `held` and `grantable`, the privileges
/// of `owner`, alone in its cell, on view `name`, with those granted it by
/// `SYSTEM_GRANTOR` for its INSTEAD OF triggers.
fn system_granted(
    catalog: &dyn Catalog,
    owner: Who<'_>,
    name: &QualifiedName,
    (mut held, mut grantable): (Actions, Actions),
) -> (Actions, Actions) {
    for p in catalog.privileges(name).into_iter().filter(|p| p.grantor == SYSTEM_GRANTOR && p.grantee.as_deref() == Some(owner.auth)) {
        held.add(&p.actions);
        if p.grantable {
            grantable.add(&p.actions);
        }
    }
    (held, grantable)
}

/// The privileges of `owner`, alone in its cell, the owner of view `view`,
/// that the view's definition gives it, and those of them grantable, as
/// `applicable` says.
fn derived(catalog: &dyn Catalog, owner: Who<'_>, view: &ViewDesc) -> (Actions, Actions) {
    let (mut held, mut grantable) = if let Some((table, columns)) = &view.updates {
        let (on_table, with_option) = applicable(catalog, owner, table);
        (through(&on_table, columns), through(&with_option, columns))
    } else {
        let select = Actions { select: true, ..Actions::default() };
        let all_grantable = view.reads.iter().all(|t| applicable(catalog, owner, t).1.select);
        (select.clone(), if all_grantable { select } else { Actions::default() })
    };
    (held.references, grantable.references) = view_references(catalog, owner, view);
    (held, grantable)
}

/// [ISO/IEC 9075:1992 §11.19 GR6b] The columns of `view` its owner `auth`
/// holds REFERENCES on, and those of them grantable: each of whose
/// underlying columns of its leaf underlying tables, base tables or views,
/// it holds REFERENCES on, grantable if all are, when it holds REFERENCES
/// on some column of every leaf underlying table. None if what the view
/// is drawn from is not recorded.
fn view_references(catalog: &dyn Catalog, owner: Who<'_>, view: &ViewDesc) -> (BTreeSet<usize>, BTreeSet<usize>) {
    let Some(u) = &view.underlying else { return (BTreeSet::new(), BTreeSet::new()) };
    let leaves: Vec<(&QualifiedName, (Actions, Actions))> = u.leaves.iter().map(|t| (t, applicable(catalog, owner, t))).collect();
    if !leaves.iter().all(|(_, (held, _))| !held.references.is_empty()) {
        return (BTreeSet::new(), BTreeSet::new());
    }
    // Whether `owner` holds REFERENCES on column `c` of leaf `t`, grantable if `grantable`.
    let holds = |(t, c): &(QualifiedName, String), grantable: bool| {
        let Some(at) = column_at(catalog, t, c) else { return false };
        leaves.iter().find(|(l, _)| *l == t).is_some_and(|(_, (held, with))| if grantable { with } else { held }.references.contains(&at))
    };
    let of =
        |grantable: bool| (0..).zip(&u.columns).filter(|(_, drawn)| drawn.iter().all(|d| holds(d, grantable))).map(|(j, _)| j).collect();
    (of(false), of(true))
}

/// The position of column `c` of table or view `t`.
fn column_at(catalog: &dyn Catalog, t: &QualifiedName, c: &str) -> Option<usize> {
    match catalog.table(t) {
        Some(table) => table.column(c),
        None => catalog.view(t)?.columns.iter().position(|column| column.name == c),
    }
}

/// [X3.135-1986 §6.6 SR3b ii] Those of every `<privilege definition>` on
/// `name` granted to an identifier `who` enables or to PUBLIC, and those of
/// them granted WITH GRANT OPTION [ISO/IEC 9075:1992 §10.3 GR2, §11.37 SR7a
/// ii; ISO/IEC 9075-2:2023 §4.42.4].
fn granted(catalog: &dyn Catalog, who: Who<'_>, name: &QualifiedName) -> (Actions, Actions) {
    let (mut held, mut grantable) = (Actions::default(), Actions::default());
    let typed = typed_as(catalog, name);
    for p in catalog.privileges(name).into_iter().filter(|p| who.holds(p.grantee.as_deref())) {
        let mut actions = p.actions.clone();
        if actions.select {
            actions.typed.methods.extend(executable_methods(catalog, typed, &p.grantor));
        }
        held.add(&actions);
        if p.grantable {
            grantable.add(&actions);
        }
    }
    // [ISO/IEC 9075-2:2023 §12.2 GR8, §11.3 GR8, §11.32 GR9, GR10] SELECT
    // WITH HIERARCHY OPTION on a supertable or a superview is SELECT on
    // it, grantable if that is; [§12.7 SR1c] and on the table/method pair
    // of each method of its type.
    for sup in super::proper_supertables(catalog, name) {
        for p in catalog.privileges(&sup).into_iter().filter(|p| p.actions.typed.hierarchy && who.holds(p.grantee.as_deref())) {
            let methods = executable_methods(catalog, typed, &p.grantor);
            held.select = true;
            held.typed.methods.extend(methods.iter().cloned());
            if p.grantable {
                grantable.select = true;
                grantable.typed.methods.extend(methods);
            }
        }
    }
    (held, grantable)
}

/// The structured type of the typed table or referenceable view `name`.
fn typed_as<'a>(catalog: &'a dyn Catalog, name: &QualifiedName) -> Option<&'a QualifiedName> {
    let typed = catalog.table(name).and_then(|t| t.typed.as_ref()).or_else(|| catalog.view(name).and_then(|v| v.typed.as_ref()));
    typed.map(|t| &t.udt)
}

/// [ISO/IEC 9075-2:2023 §12.2 GR6] Of SELECT on a table of the type
/// `typed` granted by `grantor`, the methods on whose table/method pair
/// SELECT is granted with it: each method of the type `grantor` holds
/// EXECUTE on WITH GRANT OPTION, every one of "_SYSTEM"'s. \[DESIGN\] Found
/// as it is read, not kept apart when granted, so a method added to the
/// type is one at once [§11.56 GR3] (RULES.md D81).
fn executable_methods(catalog: &dyn Catalog, typed: Option<&QualifiedName>, grantor: &str) -> BTreeSet<QualifiedName> {
    let Some(udt) = typed else { return BTreeSet::new() };
    let mut all = super::type_methods(catalog, udt);
    if grantor != SYSTEM_GRANTOR {
        all.retain(|m| super::method_executable(catalog, m, grantor));
    }
    all
}

/// The privileges on a view that `actions` on its table give, its column
/// `j` being column `columns[j]` of the table.
fn through(actions: &Actions, columns: &[usize]) -> Actions {
    Actions {
        select: actions.select,
        insert: (0..).zip(columns).filter(|(_, c)| actions.insert.contains(c)).map(|(j, _)| j).collect(),
        delete: actions.delete,
        update: (0..).zip(columns).filter(|(_, c)| actions.update.contains(c)).map(|(j, _)| j).collect(),
        // Found from what it is drawn from (`view_references`).
        references: BTreeSet::new(),
        // [ISO/IEC 9075-2:2023 §11.32 GR5] the owner's own on its view.
        trigger: false,
        // [ISO/IEC 9075-2:2023 §11.32 GR8a] Of a referenceable view, its
        // owner's by its definition (`applicable`), not by its table's.
        typed: TypedActions::default(),
    }
}
