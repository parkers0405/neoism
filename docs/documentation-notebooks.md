# Shared documentation and the welcome note

Product documentation is authored in [neoism-docs](https://github.com/parkers0405/neoism-docs), included at `vendor/neoism-docs` as a pinned Git submodule in both the application and website repositories. Its manifest supplies stable page paths and ordered navigation; its OpenAPI snapshot comes from the application's API contract. Published API reference content is generated rather than maintained by hand.

## One build, two hosting locations

The shared repository builds a self-contained static site with handbook pages, search, and a Scalar API reference. The website publishes that site alongside its landing page and art gallery. Application packaging stages the same artifact at `resources/docs`; macOS places it under the bundle's Resources directory.

**Open Documentation** starts a process-scoped, loopback-only server and opens the local site in the default browser. `neoism docs` provides the same site as a standalone foreground command, without starting graphics or the workspace daemon. All required fonts, scripts, styles, and reference data ship locally. Serving the documentation does not expose agent routes or credentials; Scalar is configured as a read-only reference.

The local routes preserve `/api/docs/`, `/api/docs/guides/.../`, and `/api/docs/reference/`. There is no runtime HTML scraping, mandatory website connection, or user-specific documentation export.

## The agent's documentation tool

The shared repository generates `bundle/content.json` from its manifest, Markdown sources, and OpenAPI contract. This is a checked-in generated resource, not a second authored source. The site's `content.json` is byte-identical. API operations and schemas are split into bounded, searchable pages alongside the handbook.

`neoism-product-docs` embeds that bundle at build time. Its build script verifies guide bodies, titles, version, and the API digest against the pinned sources, and checks that the reference matches the application's OpenAPI contract. Stale generated documentation or an incompatible API snapshot fails clearly. Cargo builds do not need Node or a website build. `NeoismDocumentationService` provides the existing `list`, `search`, and `read` operations, and tool results identify the documentation version. Existing paths such as `Neoism Agent/JSON Render.md` remain stable.

The tool reads the same centrally authored snapshot even when the website is unavailable or Notes are disabled. It does not read editable user notes or silently switch to documentation for a newer runtime. Updates arrive through reviewed shared-repository revisions and consuming-project pin updates.

## A small, user-owned welcome

Fresh Notes vaults get exactly one root-level file: `Welcome.md`. Its checklist introduces workspaces, the terminal, files, and agents, with shortcut instructions beneath each task. It also covers Notes, Alt+arrow focus navigation, and local documentation access. The full handbook is no longer seeded into the vault.

Seeding uses create-only writes and a one-time marker. Existing files, earlier handbook copies, edited notes, and manually deleted welcomes remain user-owned. The migration does not recursively remove the old `Welcome/` tree or overwrite user content. Symlinked destination directories are skipped rather than followed.

## Updating documentation

1. Edit the canonical content in `vendor/neoism-docs`, or in a separate clone of the shared repository.
2. Regenerate and check `bundle/content.json` with that repository's dependency-light generator. Export API updates from the application contract rather than editing schemas independently.
3. Run the shared site's checks and build; commit and publish the shared change.
4. Update the application and website submodule pins to the reviewed commit.
5. Build and package the consumers. CI checks out pinned submodules recursively; release builds stage the generated site, not raw Markdown or Notes.

`make docs-init`, `make docs`, and `make docs-check` cover local initialization, building/staging, and artifact validation. See [build-docs.md](../scripts/build-docs.md) and the shared repository's `CONTRACT.md` for exact artifact paths and schema details.