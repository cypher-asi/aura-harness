# SWE-Bench Pro HARD-51

The 51 tasks of the HARD-51 subset of [SWE-Bench Pro V2](https://github.com/scaleapi/SWE-bench_Pro-os)
(Scale AI, MIT), imported at commit `66f92766`. Each is a change to a large
production codebase (Ansible, Open Library, Teleport, Flipt, Element, Proton's
web clients and others): 20 in Python, 20 in Go and 11 in TypeScript or
JavaScript. The prompt is the task's own instruction: the change to make and
the interfaces the hidden tests use. They are the hardest suite here.

Each task runs in its own container image, `ghcr.io/scaleapi/swe-bench_pro-v2:<instance>`,
with the project's dependencies installed. The agent works on a copy of the
repository that aura takes out of the image, so its file tools work as on any
task, and runs the project's commands in a fresh container of the image
through the task's helper:

```
sh .swe/run 'go test ./internal/config/...'
```

The helper applies the agent's changes to the container's repository and runs
the command there, with no network. The check grades the work as SWE-Bench
Pro does: the changes as one patch applied in a fresh container, then the
task's hidden tests (`tests/test.sh`), which pass only when every test the fix
must make pass passes and every test that passed before still does.

Each task folder holds `task.toml` (the prompt, and the image as `[source]`),
`workspace/.swe/` (the helper), `checks/.gate_checks/` (the hidden tests and
the grader) and `solution/gold_patch.diff` (the reference fix).

## What it needs

Docker, with the user running aura able to use it. Each image is 1 to 5 GB;
aura pulls a task's image the first time the task runs, which can take
minutes, and copies its repository into `AURA_HOME/evals/sources`. To free the
space afterwards:

```
docker images ghcr.io/scaleapi/swe-bench_pro-v2 -q | xargs docker rmi
```

## Running them

Choose "example coding tasks: swe_bench_pro_hard" in the BENCH tab's form, or
in the terminal `/compare --suite swe_bench_pro_hard --examples 10`. The
comparison view's "cost per fix" row divides each arm's whole spend, failed
runs included, by its check passes. From the shell:

```
aura eval run evals/swe_bench_pro_hard --model gate --arm gate --budget-usd 50
aura eval run evals/swe_bench_pro_hard --model claude/claude-opus-5-5 --arm opus --budget-usd 100
aura eval compare --suite swe_bench_pro_hard --arm opus --arm gate
```

These tasks are public, and may have been in the models' training data. A
routing saving measured here is worth confirming on a few bugs from your own
code before relying on it.

## Importing again

`scripts/import_swe_bench_pro.py` downloads the pinned SWE-Bench Pro commit and
writes the tasks (`--instance <id>` for one task, `--all` for every V2 task
instead of HARD-51). To check that tasks fail as shipped and pass with the
reference fix (this pulls their images):

```
AURA_SWE_BENCH_PRO_TASKS=nodebb-8168c6c4,flipt-2ca5dfb3 \
    cargo test --test eval_suite swe_bench_pro -- --include-ignored
```

## Tasks

| Task | Language | Title | Upstream |
| --- | --- | --- | --- |
| `ansible-40ade1f8` | Python | Mount information cannot be gathered from most of a host's mount sources | [commit](https://github.com/ansible/ansible/commit/40ade1f84b8bb10a63576b0ac320c13f57c87d34) |
| `ansible-b748edea` | Python | Lack of a public helper to build multipart/form-data request bodies | [commit](https://github.com/ansible/ansible/commit/b748edea457a4576847a10275678127895d2f02f) |
| `ansible-c1f2df47` | Python | BIG-IP message routing routes cannot be managed from Ansible | [commit](https://github.com/ansible/ansible/commit/c1f2df47538b884a43320f53e787197793b105e8) |
| `ansible-d33bedc4` | Python | Config value type coercion loses tags and handles several input types inconsistently | [commit](https://github.com/ansible/ansible/commit/d33bedc48fdd933b5abd65a77c081876298e2f07) |
| `ansible-d62496fe` | Python | The human_to_bytes filter converts malformed size strings instead of rejecting them | [commit](https://github.com/ansible/ansible/commit/d62496fe416623e88b90139dc7917080cb04ce70) |
| `ansible-d72025be` | Python | nxos_interfaces does not handle enabled/shutdown correctly with device defaults | [commit](https://github.com/ansible/ansible/commit/d72025be751c894673ba85caa063d835a0ad3a8c) |
| `ansible-de5858f4` | Python | Collection operations repeatedly fetch the same metadata | [commit](https://github.com/ansible/ansible/commit/de5858f48dc9e1ce9117034e0d7e76806f420ca8) |
| `ansible-e9e60012` | Python | WinRM Kerberos: Obtaining the TGT with kinit fails or is inconsistent depending on the environment and the… | [commit](https://github.com/ansible/ansible/commit/e9e6001263f51103e96e58ad382660df0f3d0e39) |
| `element-web-5dfde12c` | TypeScript | Session information and voice broadcast state can be unreliable | [commit](https://github.com/element-hq/element-web/commit/5dfde12c1c1c0b6e48f17e3405468593e39d9492) |
| `element-web-66d0b318` | TypeScript | Add seekbar support for voice broadcast playback | [commit](https://github.com/element-hq/element-web/commit/66d0b318bc6fee0d17b54c1781d6ab5d5d323135) |
| `element-web-772df302` | TypeScript | Device management does not support signing out of multiple sessions at once | [commit](https://github.com/element-hq/element-web/commit/772df3021201d9c73835a626df8dcb6334ad9a3e) |
| `element-web-ecfd1736` | TypeScript | Combine search results when the query is present in multiple successive messages | [commit](https://github.com/element-hq/element-web/commit/ecfd1736e5dd9808e87911fc264e6c816653e1a9) |
| `flipt-2ca5dfb3` | Go | Support multiple metrics exporters (Prometheus, OpenTelemetry) | [commit](https://github.com/flipt-io/flipt/commit/2ca5dfb3513e4e786d2b037075617cccc286d5c3) |
| `flipt-3d5a345f` | Go | Make tracing sampling ratio and context propagators configurable | [commit](https://github.com/flipt-io/flipt/commit/3d5a345f94c2adc8a0eaa102c189c08ad4c0f8e8) |
| `flipt-524f2773` | Go | Support dual-format segment targeting in feature flag rules | [commit](https://github.com/flipt-io/flipt/commit/524f277313606f8cd29b299617d6565c01642e15) |
| `flipt-c1728053` | Go | Add CUE-based validation of Flipt feature configuration YAML | [commit](https://github.com/flipt-io/flipt/commit/c1728053367c753688f114ec26e703c8fdeda125) |
| `flipt-c188284f` | Go | OCI storage cannot authenticate against AWS ECR registries | [commit](https://github.com/flipt-io/flipt/commit/c188284ff0c094a4ee281afebebd849555ebee59) |
| `flipt-cd18e54a` | Go | Default Flipt configuration does not validate against the CUE schema | [commit](https://github.com/flipt-io/flipt/commit/cd18e54a0371fa222304742c6312e9ac37ea86c1) |
| `flipt-e2bd19da` | Go | Flag lookups are cached only in the gRPC interceptor and clients cannot opt out of cached values | [commit](https://github.com/flipt-io/flipt/commit/e2bd19dafa7166c96b082fb2a59eb54b4be0d778) |
| `navidrome-69e0a266` | Go | Remove size from public image ID JWT | [commit](https://github.com/navidrome/navidrome/commit/69e0a266f48bae24a11312e9efbe495a337e4c84) |
| `navidrome-874b17b8` | Go | Password change lacks current password verification | [commit](https://github.com/navidrome/navidrome/commit/874b17b8f614056df0ef021b5d4f977341084185) |
| `navidrome-bf2bcb12` | Go | Unset timestamp fields cause internal errors after upgrade from 0.50.2 to 0.51.0 | [commit](https://github.com/navidrome/navidrome/commit/bf2bcb12799b21069f137749e0c331f761d1f693) |
| `nodebb-8168c6c4` | JavaScript | Uploaded group and user cover and profile images are not fully cleaned up from disk when removed or on acco… | [commit](https://github.com/NodeBB/NodeBB/commit/8168c6c40707478f71b8af60300830fe554c778c) |
| `openlibrary-0dc5b20f` | Python | Import alternate-script author names | [commit](https://github.com/internetarchive/openlibrary/commit/0dc5b20fa186f9714f8a838178597e69f549d026) |
| `openlibrary-322d7a46` | Python | Search index updates cannot be handled per record type or inspected afterwards | [commit](https://github.com/internetarchive/openlibrary/commit/322d7a46cdc965bfabbf9500e98fde098c9d95b2) |
| `openlibrary-53d376b1` | Python | Match authors on alternate_names/surname with birth/death date | [commit](https://github.com/internetarchive/openlibrary/commit/53d376b148897466bb86d5accb51912bbbe9a8ed) |
| `openlibrary-630221ab` | Python | Backend support for "Best Book Awards" is missing (API endpoints and read-prerequisite validation) | [commit](https://github.com/internetarchive/openlibrary/commit/630221ab686c64e75a2ce253c893c033e4814b2e) |
| `openlibrary-6e889f4a` | Python | Imported book data can lose language and page accuracy | [commit](https://github.com/internetarchive/openlibrary/commit/6e889f4a733c9f8ce9a9bd2ec6a934413adcedb9) |
| `openlibrary-7cbfb812` | Python | Observation ordering helper, catalog accessor, label lookup, and persistence are missing | [commit](https://github.com/internetarchive/openlibrary/commit/7cbfb812ef0e1f9716e2d6e85d538a96fcb79d13) |
| `openlibrary-a48fd6ba` | Python | Legacy parsing of search service output is fragile and hard to maintain | [commit](https://github.com/internetarchive/openlibrary/commit/a48fd6ba9482c527602bc081491d9e8ae6e8226c) |
| `openlibrary-b67138b3` | Python | Incomplete and inconsistent extraction of MARC 880 alternate script fields | [commit](https://github.com/internetarchive/openlibrary/commit/b67138b316b1e9c11df8a4a8391fe5cc8e75ff9f) |
| `qutebrowser-3e21c821` | Python | KeySequence Type Safety and Qt6 Compatibility Issues | [commit](https://github.com/qutebrowser/qutebrowser/commit/3e21c8214a998cb1058defd15aabb24617a76402) |
| `qutebrowser-c09e1439` | Python | Successful process data is retained indefinitely | [commit](https://github.com/qutebrowser/qutebrowser/commit/c09e1439f145c66ee3af574386e277dd2388d094) |
| `qutebrowser-deeb15d6` | Python | Incorrect handling of numeric increment/decrement in URLs containing encoded characters and edge cases in i… | [commit](https://github.com/qutebrowser/qutebrowser/commit/deeb15d6f009b3ca0c3bd503a7cef07462bd16b4) |
| `qutebrowser-e70f5b03` | Python | Required message's improvements for process | [commit](https://github.com/qutebrowser/qutebrowser/commit/e70f5b03187bdd40e8bf70f5f3ead840f52d1f42) |
| `teleport-1316e672` | Go | Improve event storage and time-based search efficiency | [commit](https://github.com/gravitational/teleport/commit/1316e6728a3ee2fc124e2ea0cc6a02044c87a144) |
| `teleport-2b15263e` | Go | Assistant token usage can be inaccurate | [commit](https://github.com/gravitational/teleport/commit/2b15263e49da5625922581569834eec4838a9257) |
| `teleport-65438e6e` | Go | Touch ID credentials that fail to register remain on the device and cannot be used | [commit](https://github.com/gravitational/teleport/commit/65438e6e44b6ce51458d09b7bb028a2797cfb0ea) |
| `teleport-7744f72c` | Go | Add auditd integration | [commit](https://github.com/gravitational/teleport/commit/7744f72c6eb631791434b648ba41083b5f6d2278) |
| `teleport-eda668c3` | Go | kubectl exec and port-forward requests use the right dialer | [commit](https://github.com/gravitational/teleport/commit/eda668c30d9d3b56d9c69197b120b01013611186) |
| `teleport-f432a71a` | Go | Cryptographic key operations in the auth service have no common key store abstraction | [commit](https://github.com/gravitational/teleport/commit/f432a71a13e698b6e1c4672a2e9e9c1f32d35c12) |
| `tutanota-f373ac38` | TypeScript | Non legacy mail details fail to decrypt during loading | [commit](https://github.com/tutao/tutanota/commit/f373ac3808deefce8183dad8d16729839cc330c1) |
| `vuls-5af1a227` | Go | Incorrect detection of running kernel package versions when multiple variants are installed | [commit](https://github.com/future-architect/vuls/commit/5af1a227339e46c7abf3f2815e4c636a0c01098e) |
| `vuls-ca3f6b1d` | Go | Support Amazon Linux 2 Extra Repository | [commit](https://github.com/future-architect/vuls/commit/ca3f6b1dbf2cd24d1537bfda43e788443ce03a0c) |
| `vuls-e1fab805` | Go | Kernel vulnerability detection can include inactive kernels | [commit](https://github.com/future-architect/vuls/commit/e1fab805afcfc92a2a615371d0ec1e667503c254) |
| `vuls-ef2be3d6` | Go | Red Hat OVAL Matcher Drops Per-Package Fix State and Mishandles Modular Variants | [commit](https://github.com/future-architect/vuls/commit/ef2be3d6ea4c0a13674aaab08b182eca4e2b9a17) |
| `webclients-08bb0991` | TypeScript | TOTP verification codes are difficult to enter and review in the current input | [commit](https://github.com/protonmail/webclients/commit/08bb09914d0d37b0cd6376d4cab5b77728a43e7b) |
| `webclients-5f0745dd` | TypeScript | Bitcoin payment experience feels overly complex and hard to follow | [commit](https://github.com/protonmail/webclients/commit/5f0745dd6993bb1430a951c62a49807c6635cd77) |
| `webclients-715dbd4e` | TypeScript | Improve encryption handling for WKD contacts with X-Pm-Encrypt-Untrusted | [commit](https://github.com/protonmail/webclients/commit/715dbd4e6999499cd2a576a532d8214f75189116) |
| `webclients-c5a2089c` | TypeScript | Repeated link fetch attempts call the API again after a prior failure | [commit](https://github.com/protonmail/webclients/commit/c5a2089ca2bfe9aa1d85a664b8ad87ef843a1c9c) |
| `webclients-e65cc5f3` | TypeScript | Mailbox element list reloads at the wrong time and keeps showing stale content | [commit](https://github.com/protonmail/webclients/commit/e65cc5f33719e02e1c378146fb981d27bc24bdf4) |

The tasks, tests and reference fixes are SWE-Bench Pro's (MIT); each
repository's code keeps its own license.
