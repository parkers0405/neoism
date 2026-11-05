# Neoism documentation

Neoism's handbook and Scalar API reference share one source: [neoism-docs](https://github.com/parkers0405/neoism-docs). The native application and website include pinned revisions of that repository. The agent's `docs` tool reads the matching machine-readable bundle, including searchable API operations and schemas.

## Read the handbook

In Neoism, press Alt+P and choose **Open Documentation**. The application opens its bundled documentation in your browser through a loopback-only local server. You can also run:

```sh
neoism docs
```

The standalone command keeps its documentation server running until you press Ctrl+C. It does not start graphics or a workspace daemon. Choose the handbook for workflows and shortcuts, or **API reference** for endpoints and schemas. The packaged site works offline, including its Scalar assets.

The public copy is at <https://neoism.ai/api/docs/>. The installed documentation is pinned to the application's bundled snapshot; the online copy may be newer.

## Local-first by default

Neoism's application, workspace services, and agent runtime run locally by default. Model inference uses your configured provider, and remote providers or MCP services have their own data handling and possible fees. Local documentation access sends no workspace files, prompts, or credentials to the website.

New users receive one editable `Welcome.md` at the root of their default vault, with a getting-started checklist, shortcut instructions, and a documentation pointer. Earlier handbook copies and other existing notes are preserved. Editing or deleting those notes does not affect the authoritative handbook or agent documentation tool.

## Contribute

Initialize the pinned shared checkout with `git submodule update --init --recursive vendor/neoism-docs`. Edit guides in that repository, regenerate its machine-readable bundle, and submit changes there. Both consuming projects update their submodule pins after the documentation change is published; builds do not silently fetch a newer revision.

Run `make docs` to build and stage the site for local use. See [the shared documentation architecture](documentation-notebooks.md), [build and packaging](../scripts/build-docs.md), and the submodule's `CONTRACT.md` for the artifact format and contribution checks. This directory retains application-specific implementation references; it is not a second copy of the product handbook.