//! Abstract syntax of the statements of users (RULES.md D43). Split
//! from ast.rs (RULES.md R-SIZE-2).

/// \[EXTENSION\] A statement of users or of the database's owner (RULES.md D43), run outside a
/// transaction by the database's owner. ISO/IEC 9075:1992 has none: which
/// identifiers are authorization identifiers is implementation-defined
/// [§5.4; Annex B, item 41].
#[derive(Debug)]
pub enum UserStatement {
    /// `<user definition>` ::= CREATE USER `<authorization identifier>`
    /// \[WITH\] PASSWORD `<character string literal>`
    Create {
        /// The `<authorization identifier>`, and where it begins.
        name: (String, usize),
        /// The password, the literal's value.
        password: String,
    },
    /// `<alter user statement>` ::= ALTER USER `<authorization identifier>`
    /// \[WITH\] PASSWORD `<character string literal>`
    Alter {
        /// The `<authorization identifier>`, and where it begins.
        name: (String, usize),
        /// The new password.
        password: String,
    },
    /// `<drop user statement>` ::= DROP USER `<authorization identifier>`
    Drop {
        /// The `<authorization identifier>`, and where it begins.
        name: (String, usize),
    },
    /// `<alter database statement>` ::= ALTER DATABASE OWNER TO
    /// `<authorization identifier>`: the database handed on to the user.
    Owner {
        /// The `<authorization identifier>`, and where it begins.
        name: (String, usize),
    },
}
