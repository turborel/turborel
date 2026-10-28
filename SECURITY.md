# Security

Report a vulnerability privately, never in a public issue: on the
repository's **Security** tab, choose **Report a vulnerability**. Only the
maintainers see the report.

What counts, for a database:

- reading or changing data without the privilege for it, or escaping a
  view's or a constraint's limits;
- authentication bypassed, or a password, or anything derived from one,
  disclosed;
- TLS not used, or not verified, where it was asked for;
- a client, or a file the database reads, that can crash the server, run
  code, or take memory or disk without bound.

A wrong result, data loss with no attacker, or a crash only the database's
owner can cause is a bug: report it with the **Bug** form, its impact in
the title (CONTRIBUTING.md, Reporting issues).

Say in the report:

- what an attacker needs, such as a network connection, a user, or a
  privilege;
- what they gain;
- the commit you tested (`git rev-parse --short=12 HEAD`);
- the smallest steps that show it.

You will hear back within a week. The fix is made in private and
published with an advisory that credits you, unless you ask not to be
named.
