# Publish Project

To publish your project, `veryl publish` can be used.
Publising means to associate a version with a git revision.

```
$ veryl publish
[INFO ]   Publishing release (0.2.1 @ 297bc6b24c5ceca9e648c3ea5e01011c67d7efe7)
[INFO ]      Writing metadata ([path to project]/Veryl.pub)
```

`veryl publish` generates `Veryl.pub` which contains published version information like below.

```toml
[[releases]]
version = "0.2.1"
revision = "297bc6b24c5ceca9e648c3ea5e01011c67d7efe7"
```

After generating `Veryl.pub`, publishing sequence is completed by git add, commit and push.
The git branch to be committed must be the default branch because Veryl search `Veryl.pub` in the default branch.

```
$ git add Veryl.pub
$ git commit -m "Publish"
$ git push
```

If you enable automatic commit by `publish_commit` in `[publish]` section of `Veryl.toml`, git add and commit will be executed after publish.

```
$ veryl publish
[INFO ]   Publishing release (0.2.1 @ 297bc6b24c5ceca9e648c3ea5e01011c67d7efe7)
[INFO ]      Writing metadata ([path to project]/Veryl.pub)
[INFO ]   Committing metadata ([path to project]/Veryl.pub)
```

### Version Bump

You can bump version with publish at the same time by `--bump` option.
As the same as publish, `bump_commit` in `[publish]` section of `Veryl.toml` can specify automatic commit after bump version.

```
$ veryl publish --bump patch
[INFO ]      Bumping version (0.2.1 -> 0.2.2)
[INFO ]     Updating version field ([path to project]/Veryl.toml)
[INFO ]   Committing metadata ([path to project]/Veryl.toml)
[INFO ]   Publishing release (0.2.2 @ 159dee3b3f93d3a999d8bac4c6d26d51476b178a)
[INFO ]      Writing metadata ([path to project]/Veryl.pub)
[INFO ]   Committing metadata ([path to project]/Veryl.pub)
```

### Configuration

```toml
[publish]
bump_commit = true
bump_commit_message = "Bump"
```

| Configuration             | Value                | Default               | Description                                     |
|---------------------------|----------------------|-----------------------|-------------------------------------------------|
| bump_commit               | boolean              | false                 | automatic commit after bump                     |
| publish_commit            | boolean              | false                 | automatic commit after publish                  |
| bump_commit_mesasge       | string               | "chore: Bump version" | commit message after bump                       |
| publish_commit_mesasge    | string               | "chore: Publish"      | commit message after publish                    |
