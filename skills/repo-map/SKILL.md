---
name: repo-map
description: Compact tree and file-type summary of a repository (respects .gitignore). Use to orient in an unfamiliar codebase.
python: repo_map:main
hidden: false
---

# repo-map

`repo_map` is preloaded in the kernel. It is synchronous (no `await`).

```python
print(repo_map("."))                                 # depth 2, ≤ 200 lines
print(repo_map("packages/client", max_depth=4))
print(repo_map(".", max_depth=1, max_entries=50))
```

Output: a header `name: N files — .ts 120, .md 14, …`, then an indented tree.
Directories show `name/ (N files)`; entries deeper than `max_depth` are folded
into their directory's count.

Files come from `git ls-files --cached --others --exclude-standard` when the
path is inside a git work tree, so ignored files are excluded. Outside git it
walks the directory, skipping `.git`, `node_modules`, `target`, `__pycache__`,
virtualenvs, and build output.

For file contents, use `pathlib` directly on the paths it shows.
