//! [X3.135-1986 §4.15, §6.6] Privileges: what a `<privilege definition>`
//! grants, as the catalog keeps it, and the applicable privileges of an
//! `<authorization identifier>` on a table [X3.135-1986 §6.6 SR3]. Split
//! from catalog.rs (RULES.md R-SIZE-2).

use std::collections::{BTreeMap, BTreeSet};

use super::{Catalog, QualifiedName, ViewDesc};

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
}

impl Actions {
    /// Every privilege on a table of `columns` columns.
    pub fn all(columns: usize) -> Actions {
        let every = || (0..columns).collect();
        Actions { select: true, insert: every(), delete: true, update: every(), references: every() }
    }

    /// Adds the privileges of `other`.
    pub fn add(&mut self, other: &Actions) {
        self.select |= other.select;
        self.insert.extend(other.insert.iter().copied());
        self.delete |= other.delete;
        self.update.extend(other.update.iter().copied());
        self.references.extend(other.references.iter().copied());
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
        }
    }

    /// Each privilege, one by one: as SQL-92's privilege descriptors are,
    /// one for each action on the table, or on one of its columns
    /// [ISO/IEC 9075:1992 §10.3 GR3, §11.36 GR3].
    pub fn each(&self) -> Vec<Privilege> {
        let tabled = [(self.select, Privilege::Select), (self.delete, Privilege::Delete)];
        let mut each: Vec<Privilege> = tabled.into_iter().filter(|(held, _)| *held).map(|(_, p)| p).collect();
        each.extend(self.insert.iter().map(|c| Privilege::Insert(*c)));
        each.extend(self.update.iter().map(|c| Privilege::Update(*c)));
        each.extend(self.references.iter().map(|c| Privilege::References(*c)));
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
            }
        }
        actions
    }

    /// The first privilege of `other` these lack, named with the column
    /// names `columns` (or positions, where it has none): "SELECT",
    /// "UPDATE(AMT)"; none if they lack none.
    pub fn missing(&self, other: &Actions, columns: &[String]) -> Option<String> {
        let actions = [("SELECT", self.select, other.select), ("DELETE", self.delete, other.delete)];
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
        let column = other.references.difference(&self.references).next()?;
        Some(format!("REFERENCES({})", name(column)))
    }

    /// The column of the privilege `missing` names, if it names one: the
    /// inaccessible column [ISO/IEC 9075:1992 §18.1 GR3g ii].
    pub fn missing_column(&self, other: &Actions) -> Option<usize> {
        if (other.select && !self.select) || (other.delete && !self.delete) {
            return None;
        }
        let mut columns = other.insert.difference(&self.insert);
        let first = columns.next().or_else(|| other.update.difference(&self.update).next());
        first.or_else(|| other.references.difference(&self.references).next()).copied()
    }
}

/// One privilege on a table: an action on it, or on one of its columns
/// [ISO/IEC 9075:1992 §10.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
}

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
            let actions = Actions::of(each.iter().filter(|((of, _), g)| *of == pair && **g == grantable).map(|((_, p), _)| *p));
            if !actions.is_empty() {
                let (grantor, grantee) = pair.clone();
                merged.push(PrivilegeDesc { grantor, grantee, table: table.clone(), actions, grantable });
            }
        }
    }
    merged
}

/// [X3.135-1986 §6.6 SR3] The applicable privileges of `auth` on the
/// table or view `name`, and those of them that are grantable.
///
/// The owner, the `<authorization identifier>` that owns its schema
/// (`owner_of`), has every
/// privilege on a base table, all grantable \[SR3b i 1\]; on a read-only
/// view, SELECT, grantable if its SELECT on every table the view names is
/// \[SR3b i 2\]; on an updatable view, those it has on the table the view's
/// `<from clause>` names, column for column \[SR3b i 3\]. Anyone else has
/// those of every `<privilege definition>` on it whose grantees are `auth`
/// or PUBLIC \[SR3b ii\], grantable if one of them says WITH GRANT OPTION:
/// SQL-92's applicable privileges are those of PUBLIC too [ISO/IEC
/// 9075:1992 §10.3 GR2], and a grant to PUBLIC permits one by anyone
/// [§11.37 SR7a ii], where SQL-86 took only one naming `auth` itself. On a table of INFORMATION_SCHEMA, everyone has
/// SELECT, grantable, "so that SELECT privilege can be further granted on
/// views that reference" it, and no other privilege [ISO/IEC 9075:1992
/// §21.1].
pub fn applicable(catalog: &dyn Catalog, auth: &str, name: &QualifiedName) -> (Actions, Actions) {
    if name.auth == crate::system::INFORMATION_SCHEMA {
        let select = Actions { select: true, ..Actions::default() };
        return (select.clone(), select);
    }
    if super::owner_of(catalog, &name.auth) != auth {
        return granted(catalog, auth, name);
    }
    if let Some(table) = catalog.table(name) {
        let all = Actions::all(table.columns.len());
        return (all.clone(), all);
    }
    let Some(view) = catalog.view(name) else { return (Actions::default(), Actions::default()) };
    let (mut held, mut grantable) = if let Some((table, columns)) = &view.updates {
        let (on_table, with_option) = applicable(catalog, auth, table);
        (through(&on_table, columns), through(&with_option, columns))
    } else {
        let select = Actions { select: true, ..Actions::default() };
        let all_grantable = view.reads.iter().all(|t| applicable(catalog, auth, t).1.select);
        (select.clone(), if all_grantable { select } else { Actions::default() })
    };
    (held.references, grantable.references) = view_references(catalog, auth, view);
    (held, grantable)
}

/// [ISO/IEC 9075:1992 §11.19 GR6b] The columns of `view` its owner `auth`
/// holds REFERENCES on, and those of them grantable: each drawn only from
/// columns it holds REFERENCES on, grantable if all are, when it holds
/// REFERENCES on some column of every leaf underlying table. None if what
/// the view is drawn from is not known.
fn view_references(catalog: &dyn Catalog, auth: &str, view: &ViewDesc) -> (BTreeSet<usize>, BTreeSet<usize>) {
    let Some(u) = &view.underlying else { return (BTreeSet::new(), BTreeSet::new()) };
    let leaves: Vec<(&QualifiedName, (Actions, Actions))> = u.leaves.iter().map(|t| (t, applicable(catalog, auth, t))).collect();
    if !leaves.iter().all(|(_, (held, _))| !held.references.is_empty()) {
        return (BTreeSet::new(), BTreeSet::new());
    }
    // Whether `auth` holds REFERENCES on column `c` of leaf `t`, grantable if `grantable`.
    let holds = |(t, c): &(QualifiedName, String), grantable: bool| {
        let Some(at) = catalog.table(t).and_then(|d| d.column(c)) else { return false };
        leaves.iter().find(|(l, _)| *l == t).is_some_and(|(_, (held, with))| if grantable { with } else { held }.references.contains(&at))
    };
    let of =
        |grantable: bool| (0..).zip(&u.columns).filter(|(_, drawn)| drawn.iter().all(|d| holds(d, grantable))).map(|(j, _)| j).collect();
    (of(false), of(true))
}

/// [X3.135-1986 §6.6 SR3b ii] Those of every `<privilege definition>` on
/// `name` granted to `auth` or PUBLIC, and those of them granted WITH GRANT
/// OPTION [ISO/IEC 9075:1992 §10.3 GR2, §11.37 SR7a ii].
fn granted(catalog: &dyn Catalog, auth: &str, name: &QualifiedName) -> (Actions, Actions) {
    let (mut held, mut grantable) = (Actions::default(), Actions::default());
    for p in catalog.privileges(name) {
        let named = p.grantee.as_deref() == Some(auth);
        if named || p.grantee.is_none() {
            held.add(&p.actions);
        }
        if (named || p.grantee.is_none()) && p.grantable {
            grantable.add(&p.actions);
        }
    }
    (held, grantable)
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
    }
}
