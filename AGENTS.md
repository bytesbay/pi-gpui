# Notes for coding agents

## Commit messages

Release notes and the changelog are generated from commit subjects, so every
commit on `main` follows Conventional Commits:

```
type(scope): subject
```

- **Lowercase, imperative, no trailing period, at most about 72 characters.**
  `fix(theme): load a theme file without a name instead of crashing`, not
  `Fixed crash.`
- **Write the subject for users of the app.** Say what changed for them, not
  which function moved: `feat(tabs): add close all button when several tabs are
  open`. The subject becomes a changelog line as it is.
- **One change per commit.** Two features or a feature and a fix are two
  commits, so each gets its own line.

Types and where they end up:

| Type | Use for | In the changelog |
|---|---|---|
| `feat` | something new a user can see or use | Added |
| `fix` | a bug fix a user would notice | Fixed |
| `perf` | faster or lighter, same behavior | Changed |
| `refactor` | code change without a visible effect | no |
| `docs` | README, docs/, comments | no (site texts use `docs(site)`) |
| `ci` | workflows | no |
| `chore` | releases, tooling, dependencies | no |
| `test` | tests only | no |

Scopes are optional; use the area of the app when there is a clear one:
`chat`, `tabs`, `sidebar`, `files`, `diff`, `theme`, `settings`, `plugins`,
`updater`, `linux`, `site`, `examples`, `dev`.

A change that breaks something users rely on (settings keys, plugin API,
supported pi versions) gets a `!` after the type and a `BREAKING CHANGE:` line
in the body: `feat(plugins)!: rename sessions.reload to sessions.refresh`.

Releases: bump `version` in `Cargo.toml`, commit it as `chore: release X.Y.Z`,
then run `script/release.sh`.
