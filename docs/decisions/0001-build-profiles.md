# 0001: Separate build modes and use ThinLTO releases

- Status: Accepted
- Decision date: 2026-11-01
- Implementation: [PR #55](https://github.com/zenshay/ssd-refresh/pull/55)
- Adopted commit: `98e99a9e72d8598c1891abe50d3e68ffca10719d`
- Translation: [繁體中文](0001-build-profiles.zh-TW.md)
- Follow-up: [Additional local, package, and native GUI validation](0001-build-validation.md)

## Decision

Use development settings for local edits and PR checks.
Use ThinLTO and 16 code generation units for release builds on all supported platforms.
ThinLTO is a less costly form of optimization across packages.
Code generation units split a package into work that the compiler can process in parallel.

Keep one release profile in `Cargo.toml` for local `--release` commands and remote release builds.
Keep the stable Rust toolchain, dependency versions, panic behavior, and existing correctness checks.

| Activity | Mode and boundary |
|---|---|
| Local bug fix | Development mode; reuse local incremental data. Commit does not wait for remote CI. |
| Remote synchronization | A feature branch push saves progress. A PR starts the existing checks. Merge waits for their result. |
| Release | A version tag starts checks, package builds, and draft verification. Use the release profile. |

Incremental compilation stores fine-grained data for later local builds.
PR dependency caches and local incremental data are different forms of reuse.
The manual `release_builds` input runs extra optimized platform validation. Normal PRs skip that work.

## Problem and priorities

The maintainer wants to spend time changing the project, rather than waiting for CI.
Local edit feedback, PR checks, and release packaging have different goals.
Requiring release builds for each local edit or ordinary PR would add work without matching those goals.

The release profile previously used fat LTO and one code generation unit.
Windows release work was the slowest package stage in the historical release runs reviewed for this decision.

## Evidence used

The [archived evidence](evidence/2026-11-01-build-profiles.json) contains the measurements and model inputs.
It preserves sample values, source commits, toolchains, runner CPUs, and links to the original runs.
It contains no local workspace paths or storage data.

Each comparison below used one runner for its baseline and candidate.
Rebuild values are medians of three samples with incremental compilation disabled.
The workload changes a comment in shared Rust source and rebuilds the GUI, CLI, and a synthetic example.
It is not a measurement of every real edit or the complete release workflow.
Runtime loops process 8 MiB of synthetic SHA256 input or 10,000 CSV rows for at least 700 ms.
Compiler memory is sampled every 200 ms. It is not a guaranteed peak for the whole machine.

| Candidate | Its paired baseline | Candidate rebuild | Saving |
|---|---:|---:|---:|
| ThinLTO, one unit | 138.313 s | 123.872 s | 14.441 s; 10.4% |
| ThinLTO, 16 units | 169.004 s | 125.070 s | 43.934 s; 26.0% |

The two comparisons used different CPU models. Do not compare their baseline times as a setting effect.
For 16 units, the cold build sample decreased from 356.393 s to 317.488 s, or 10.9%.

The selected candidate had these measured costs:

- GUI size increased by 1,195,008 bytes: 1.14 MiB, or about 1.20 MB.
- Compiler memory increased by 453 MiB in sampled working sets.
- Temporary build output increased by 264,468,563 bytes. This is not the uploaded GitHub cache size.
- Parsing 10,000 synthetic CSV rows took 0.429 ms more, or 5.6%, in nine alternating samples per condition.
- The synthetic SHA256 result and row checks passed. SHA256 throughput was almost unchanged.

The CSV result does not mean the whole application or SSD operations became 5.6% slower.
The benchmark does not measure GUI startup or real storage operations.

## Why this tradeoff was accepted

The maintainer accepted the small absolute size and synthetic runtime costs in exchange for shorter builds.
Initial percentage thresholds were screening aids, not automatic rejection rules.
Judge additional milliseconds, bytes, memory, and total waiting time together.
Correctness failures remain a reason to reject a candidate.

The 14-second candidate remains a conservative alternative. It was not the selected configuration.
Development code generation changes did not shorten full checks enough to justify adoption.
Nightly threading added maintenance and memory costs without a useful paired build improvement.
Cranelift failed the existing panic cleanup test on Windows. Its failure was not bypassed.
Disabling LTO had larger synthetic runtime costs and lacked a follow-up paired confirmation.

## Release waiting model and its limits

Historical warm-cache Windows jobs took 260 s for x64 and 289 s for ARM64.
ARM64 compilation accounted for 212 s of its job.
Parallel jobs finish when the slowest job finishes; their durations must not be added together.
Speeding up only x64 would leave the ARM64 wait unchanged in that example.

A model combines 258 s of checks, 289 s of package work, and 39 s of historical overhead.
Its baseline is 586 s. These inputs come from different runs and commits.
If both Windows jobs save 44 s, the model saves 7.5% overall.
If both compilation stages save 26%, the model saves about 55 s, or 9.4% overall.
These are estimates. The complete optimized version-tag workflow has not been timed.

## Validation after implementation

[The final manual run](https://github.com/zenshay/ssd-refresh/actions/runs/41281946938) passed all 12 active jobs.
It covered the existing full checks, Windows x64 optimized builds and tests, and ARM64 builds and test compilation.
Linux and macOS optimized CLI builds and tests also passed.
All eight ordinary PR checks passed before merge.

[The main run](https://github.com/zenshay/ssd-refresh/actions/runs/41282453857) passed all 10 active jobs after merge.
Both Windows release dependency caches were prepared successfully.
No version tag or release was created for this validation.

Cross-compilation is not an ARM64 hardware test.
Real storage, privileged disk-image tests, power-loss recovery, and local rebuild performance were not measured in this change.

## Adoption and maintenance costs

| Stage | Cost or trigger | Evidence and limits |
|---|---|---|
| First adoption | Compiler setting changes can invalidate caches and require rebuilding dependencies. | Main cache preparation passed; no warm-cache speed guarantee follows from that result. |
| Daily use | Local edits retain development mode. Ordinary PRs skip extra release validation. | No commit or push hook was added. |
| Maintenance | Keep local and remote release settings in the root manifest. | No nightly compiler, cache service, or paid runner was added. |
| Environment change | Recheck measurements after Rust, dependency, runner, or target changes. | Old measurements do not establish future speed. |
| Reversal | Restore `lto = true` and `codegen-units = 1` in the release profile. | Caches may rebuild. Application data formats do not change. |

## External context

Sources below were reviewed on 2026-11-01. They are examples, not a survey of community adoption.

- [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) define separate development, test, and release modes.
- [Tauri settings](https://github.com/tauri-apps/tauri/blob/dev/Cargo.toml) favor small release binaries and use fat LTO with one unit.
- [Zed settings](https://github.com/zed-industries/zed/blob/main/Cargo.toml) separate development and release modes, with ThinLTO and a 16-unit main-package override.

The examples support separating modes by purpose. They do not prove that one profile is best for every Rust desktop project.
Do not copy another project's panic-abort setting. This project requires cleanup after panics.
