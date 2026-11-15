# cdsk — storage for the VM/GPU system

`cdsk` is the storage software behind `nixstorage`'s declarations: dataset
shape convergence, ownership convergence, instance disks, and a read-only
layout check. It is a library (`cdsk`) and a binary (`cdsk`) that computes
plans; running a plan needs an explicit `--apply`.

## Purpose

- **Dataset shape.** A `nixstorage`-style declaration assigns datasets to
  classes whose `recordsize`, `compression` and extra properties are the
  contract. `cdsk` compares them with observed ZFS state and plans only the
  `zfs set` calls that close real drift, including datasets declared
  `children = "reconcile"`.
- **Ownership and top-directory mode.** The port of `nixstorage`'s
  `reconcile.sh`: roots first, leaves shallowest first, `chown -h` only for
  mis-owned paths, `chmod` on the top directory only, declared carve-outs
  reported and never touched.
- **Instance disks.** A 4Kn zvol (`volblocksize=4096`), a clone of a base
  image's `@clean` snapshot, snapshots under class-understood names, and reset
  to `@clean` by `zfs rollback -r`. Also implements the disk-reset seam that
  `cwld` leaves open (`DiskReset`).
- **Layout verify.** `sfdisk --json` against a declared partition table:
  names, type GUIDs, sizes and sector size, read-only.
- **Save images.** A rollback removes the managed save it makes worthless; a
  failed restore keeps its image and raises an event.

ZFS is read through the `zfs` CLI's JSON output (OpenZFS 2.3+), never through
libzfs. The [`Zfs`](src/zfs.rs), [`Fs`](src/fs.rs) and
[`PartitionTables`](src/layout.rs) traits are the whole observation surface, so
tests and the CLI's `--state` fixture stand in for a live host.

## Boundaries — this crate must never

- create or destroy a dataset during convergence; an absent dataset is
  reported as drift and left alone,
- destroy a snapshot or a dataset at all: a reset is a rollback on the
  instance's own dataset, and the golden image's `@clean` is never a rollback
  target; a dataset that is not a clone of the declared base is refused,
- create a zvol with any geometry other than logical 4096 and physical 4096;
  512 and 512e are refused, never substituted,
- write to a block device while verifying a layout; `sfdisk --json` only
  reads,
- follow a symlink when changing ownership (`chown -h`, always), or `chmod`
  recursively,
- hold host data: pool, dataset, device and path names in the repository are
  synthetic, identity resolution (a `nixiam` name to a uid/gid) stays in the
  Nix declaration.

## Usage

```sh
# print the zfs calls and ownership changes for a declaration
cdsk plan converge --decl fixtures/declaration.json --state fixtures/state.json

# print the rollback (and save-image removal) for one instance
cdsk plan reset example-vm --state fixtures/state.json
```

Both commands print one command per line on standard output and the events as
JSON lines on standard error. Nothing is executed without `--apply`.

In a deployment, `--state` is produced from the host's own `zfs get -j`,
`zfs list -j` and `lstat` output plus the instance table (private, in
`corbet-infra/instances`); the fixture in `fixtures/state.json` shows the
shape with synthetic names.

## How to run the tests

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The integration tests (`tests/it/`) cover shape drift (only drifted properties
are set), recursive children, ownership idempotence, prune and carve-outs, 4Kn
refusal of 512/512e, clone/snapshot/reset sequences against a fake `Zfs`,
golden-snapshot protection, layout pass/fail/skip, and both save-image rules.
No test touches a pool or a real filesystem.

## License

`LGPL-3.0-only WITH LGPL-3.0-linking-exception`; see `LICENSE`.
