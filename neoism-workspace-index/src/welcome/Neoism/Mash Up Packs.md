# Mash Up Packs

Mash Up Packs bundle coordinated visual slots such as an IDE theme, shader overlay, filters, fonts, wallpaper, scrollbars, Markdown styling, and icons. A pack may also select native desktop editor Lua packages. Agent and daemon plugins are not affected.

## Baseline restoration

Activating the first pack captures your current effective theme and font under the typed `appearance.mashup-baseline` setting. Switching directly from pack A to pack B preserves that original baseline: every slot comes from B when B supplies it, otherwise from the baseline, never from A. Choosing **None** restores both baseline values and clears the active pack and baseline together. A null baseline font is meaningful and restores the normal font cascade.

Neoism writes pack ID, baseline, theme, and font as one crash-atomic JSONC-preserving replacement. An old configuration that has an active pack but no baseline is left untouched at startup; its current effective theme and font are conservatively captured only on the next explicit pack transition. With no active pack, choosing None is a visual no-op and clears any stale baseline.

A direct desktop theme-picker choice made while a pack is active updates the baseline restoration theme in the same atomic write. Packs do not silently recapture a baseline during startup. Manual edits through arbitrary external or web settings paths are not all baseline-aware, so edit `appearance.mashup-baseline` as well when intentionally changing an active pack's later restoration target.

## Pack manifest

Each pack lives under `~/.config/neoism/packs/<id>/` with a `pack.json` manifest. JSONC comments and trailing commas are accepted.

```jsonc
{
  "pack": {
    "name": "Focused Writing",
    "description": "A quiet prose workspace",
    "theme": "focused-writing",
    "font-family": "Iosevka"
  },
  "editor-plugins": {
    "mode": "only",
    "enabled": ["dev.example.prose", "dev.example.spellcheck"],
    "disabled": ["dev.example.code-minimap"]
  }
}
```

`overlay` keeps the normal globally eligible editor packages and subtracts `disabled`. `only` admits the `enabled` roots and their complete dependency closure. Neoism trims IDs, drops blanks, sorts and deduplicates lists, and rejects an effective declaration that both enables and disables the same ID.

An `only` root that is not installed rejects the candidate. Missing IDs in `overlay` are harmless. A missing or disabled dependency causes the normal plugin graph failure, so Neoism retains the last working plugin generation rather than activating a partial graph.

## Native surface backgrounds

Packs and Lua use the same retained `StylePatch` data. This example keeps Pastel Dark, explicitly makes the Agent composer black, and adds sparse animated white stars behind its text:

```json
{
  "pack": {
    "name": "Quiet Stars",
    "theme": "pastel_dark"
  },
  "ui": {
    "styles": {
      "composer.agent": {
        "background": "#000000",
        "background_effects": [
          { "kind": "stars", "color": [1, 1, 1, 1], "seed": 17, "density": 0.6, "speed": 0.6, "opacity": 0.85 }
        ]
      }
    }
  }
}
```

`ui.styles` is a top-level map with the same snake_case fields as `neoism.ui.style(selector, patch)`. Effects default off. Omitted/null effects inherit; `[]` disables them. Fields merge in priority order: accepted pack, editor packages, personal `init.lua`. Dotted parents inherit broad to specific within each layer. The accepted pack's styles publish with its look snapshot; changing packs or choosing None clears the previous pack layer, while rejected pack-apply candidates keep the last working snapshot.

Native effect surfaces are `composer.agent`, `chrome.top` (the actual action strip, including left/right/bottom docks), `status`, `editor.code`, and `file-tree`. `neoism.ui.surfaces()` reports this list. Stars include analytically contained glow/antialiasing footprints inside the caller's rounded bounds; foreground text and controls paint afterward. A second variant, `scanlines`, paints restrained moving dashes using the same bounded geometry and clipping. These are extensible native variants, not arbitrary Lua graphics or paint callbacks. Unrecognized effects/fields and invalid bounds reject the candidate. Custom style selectors remain allowed but do not create new render surfaces.

Each effect accepts `kind`, RGBA `color` (four finite channels 0..1), unsigned 32-bit `seed`, finite `density` and `speed` (0..4), and finite `opacity` (0..1). Defaults are white, seed 0, density/speed/opacity 1. At most four effects are allowed; native paint is capped at 384 primitives per surface and 1536 per window frame. `speed = 0` pauses animation; zero density/opacity disables paint. Animation demand resets every window frame and exists only for visible painted animated primitives. Hidden, disabled, and fully occluded surfaces add no idle redraw demand.

For personal customization, use `neoism.ui.style("composer.agent", { background_effects = {} })` to disable pack stars, or publish another typed effect list. Background overrides are consumed by these draw sites; other `StylePatch` properties are site-specific, not universal CSS. A true terminal-grid background remains deferred: quads drawn after terminal cells cannot be a genuine background. Desktop owns the effect frame lifecycle; web animation integration is not enabled by this change.

## User overrides

Activate a pack under `appearance.mashup-pack`. Replace selected plugin fields under `plugins.mashup-overrides`; omitted fields inherit the pack value and an explicit empty array clears a list.

```jsonc
{
  "appearance": {
    "mashup-pack": "focused-writing"
  },
  "plugins": {
    "disabled": ["dev.example.never-run"],
    "mashup-overrides": {
      "focused-writing": {
        "mode": "overlay",
        "enabled": [],
        "disabled": []
      }
    }
  }
}
```

Global policy remains authoritative. `plugins.disabled` is a hard veto, and packs cannot grant capabilities, approve native code, bypass trusted source checks, or affect Agent plugins. If there is no active pack, the pack is missing, or neither the pack nor its override declares editor plugins, normal global editor plugin behavior is unchanged.

## Reload behavior

Picker and modal input only queue the requested pack ID. The application-owned pump resolves that exact manifest once, derives the candidate configuration and selection, discovers and validates the graph, and builds eager runtimes before changing configuration or visuals. This is a last-known-good transaction: resolver and discovery failures retain the current visuals, configuration, Lua manager, and immutable snapshot. Excluded package Lua does not run or enter the snapshot. The Extensions inventory keeps discovered exclusions visible as controlled by the active Mash Up Pack and does not offer the global Enable action for them.

After validation, Neoism writes the active pack, baseline, resolved theme, and resolved font together, then atomically publishes the accepted editor Lua generation and the already-resolved visual slots in the same application operation. Deactivation follows the same transaction, restores baseline visuals, and restores ordinary global editor-plugin behavior. The watcher reload caused by the write resolves the same policy and is idempotent. Web-triggered pack application uses the same backend transition resolver and single writer; web receives appearance only because editor Lua runs on desktop.

Wallpaper decoding and shader setup are the fallible visual steps and run before theme, filter, font, and look publication. A synchronous failure restores the prior wallpaper where needed, rolls back the persisted fields, and leaves the old Lua generation active. Neoism logs a failed rollback attempt; graphics failures reported only after a deferred GPU upload has already been accepted cannot be synchronously rolled back.

## Bundled Lucid Blocks showcase

`lucid-blocks` is a first-party Alice-in-Wonderland / looking-glass pack with an original midnight card-table theme, semantic icon accents, and a restrained text-safe edge/checker/chromatic shader. Its `editor-plugins` policy uses `overlay` and enables `io.neoism.lucid-birds`; the visual pack remains fully usable if that package is unavailable or denied.

Every focused Agent composer edit immediately advances `AgentChanged.composerRevision`. The package emits one independently seeded vector bird for every revision. Bird geometry, wing flap weights, theme colors, and physics ranges live entirely in Lua; Rust validates the generic particle payload, launches it above the composer, owns every trajectory and frame, and retires it within three seconds. Rapid typing creates overlapping birds traveling in different directions without Lua running during paint, hit testing, or animation frames. Draft text is never exposed and expired particles leave no idle redraw owner. Web clients receive only the pack appearance.

The package is seeded independently at `~/.config/neoism/plugins/io.neoism.lucid-birds` with marker `io.neoism.lucid-birds@1` in `plugins/.neoism-first-party-seeds`. Seeding occurs only when the package directory is absent, never overwrites an existing directory or user edits, and does not share pack or welcome-document markers. Lucid Blocks excludes both earlier Lucid Rabbit seeds while active, so upgrading the showcase does not overwrite or duplicate an installed revision.

Pack activation never grants capabilities. To enable the desktop sequence, grant the exact capabilities requested by the manifest:

```jsonc
{
  "plugins": {
    "grants": {
      "io.neoism.lucid-birds": [
        "config.current",
        "effect.emit"
      ]
    }
  }
}
```

Without these grants, guarded API calls fail harmlessly and the Lucid Blocks theme, shader, icons, and scrollbar still load.