import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { NeoismClient, OperationResponse } from "@neoism/sdk";
import { Settings, ThemePicker } from "./Settings";
import { ProviderConnections } from "./ProviderConnections";
import {
    ProviderRows,
    providerGroups,
    PROVIDER_WINDOW,
} from "./ProviderDirectory";
import { defaultPreferences } from "../types";
import { themeOptions } from "../appearance";
import { themes } from "../generated/themes";

const client = { catalog: { providers: {} } } as NeoismClient;
const noop = () => {};
const catalog = {
    all: [
        { id: "openai", name: "OpenAI", models: {} },
        { id: "anthropic", name: "Anthropic", models: {} },
        { id: "local", name: "Local endpoint", models: {} },
    ],
    connected: ["openai"],
    default: {},
} as OperationResponse<"v2.providers.list">;

describe("OpenCode-shaped settings and provider views", () => {
    it("owns a dialog with grouped real navigation, not the shared full-width modal header", () => {
        const html = renderToStaticMarkup(
            <Settings
                client={client}
                value={defaultPreferences}
                token=""
                save={noop}
                close={noop}
            />,
        );
        expect(html).toContain('class="settings-dialog"');
        expect(html).toContain('aria-label="Settings"');
        expect(html).toContain("Desktop");
        for (const page of ["General", "Appearance", "Servers", "Providers"])
            expect(html).toContain(page);
        expect(html).not.toContain("Shortcuts");
        expect(html).not.toContain(">Models<");
        expect(html).toContain("Display name");
        expect(html).toContain("Save settings");
        expect(html).not.toContain('class="modal"');
        expect(html.match(/<form\b/g)).toHaveLength(1);
    });
    it("opens an initial provider in a separate compact settings view, with no save form", () => {
        const html = renderToStaticMarkup(
            <Settings
                client={client}
                value={defaultPreferences}
                token=""
                save={noop}
                close={noop}
                initialProviderId="openai"
            />,
        );
        expect(html).toContain("settings-connecting");
        expect(html).toContain("provider-back");
        expect(html).toContain("provider-identity");
        expect(html).toContain("Continue");
        expect(html).toContain('type="password"');
        expect(html).not.toContain("<form");
        expect(html).not.toContain("Confirm delete account");
    });
    it("keeps /connect inline, without adding a modal or nested form", () => {
        const html = renderToStaticMarkup(
            <ProviderConnections
                client={client}
                directory="/work"
                initialProviderId="anthropic"
                workspaceId="workspace"
                selectedConnection={{
                    providerId: "anthropic",
                    connectionId: "work",
                }}
                onSelectConnection={noop}
            />,
        );
        expect(html).toContain("provider-flow");
        expect(html).not.toContain("<dialog");
        expect(html).not.toContain("<form");
        expect(html).toContain("Saved accounts");
        expect(html).not.toContain("Prompt account selection is not available");
    });
    it("renders connected and popular rows with actual Connect buttons and no invented source badges", () => {
        const html = renderToStaticMarkup(
            <ProviderRows catalog={catalog} connect={noop} manage={noop} />,
        );
        expect(html).toContain("Connected");
        expect(html).toContain("Popular");
        expect(html).toContain('aria-label="Connect Anthropic"');
        expect(html).toContain('aria-label="Connect OpenAI"');
        expect(html).toContain(">Accounts</button>");
        expect(html).toContain("Local endpoint");
        expect(html).not.toContain("Environment");
        expect(html).not.toContain("API key");
        expect(html).toContain("#anthropic");
        const all = renderToStaticMarkup(
            <ProviderRows catalog={catalog} connect={noop} manage={noop} />,
        );
        expect(all).toContain("Local endpoint");
    });
    it("retains native Pastelbeans and every generated palette", () => {
        expect(themeOptions.find((t) => t.id === "pastelbeans")?.colors.bg).toBe(
            "#151515",
        );
        for (const theme of themes)
            expect(themeOptions.some((t) => t.id === theme.id)).toBe(true);
        expect(themes.length).toBe(101);
    });
});

describe("searchable settings catalogs", () => {
    const large = {
        ...catalog,
        all: [
            ...catalog.all,
            ...Array.from({ length: 80 }, (_, i) => ({
                ...catalog.all[0],
                id: `custom-${i}`,
                name: `Endpoint ${i}`,
                description: `Description ${i}`,
                models: {},
            })),
            catalog.all[0],
        ],
    };
    it("searches names, ids and descriptions outside the first reveal window", () => {
        expect(
            providerGroups(large)
                .groups.flatMap((g) => g.items)
                .some((p) => p.id === "custom-79"),
        ).toBe(false);
        for (const query of ["Endpoint 79", "CUSTOM-79", "Description 79"])
            expect(
                providerGroups(large, query)
                    .groups.flatMap((g) => g.items)
                    .map((p) => p.id),
            ).toEqual(["custom-79"]);
    });
    it("partitions connected, popular and remainder without duplicates and reaches the end", () => {
        const first = providerGroups(large);
        expect(first.hasMore).toBe(true);
        expect(first.groups[2].items).toHaveLength(PROVIDER_WINDOW);
        const last = providerGroups(large, "", 120);
        const ids = last.groups.flatMap((g) => g.items.map((p) => p.id));
        expect(new Set(ids).size).toBe(ids.length);
        expect(ids).toHaveLength(83);
        expect(last.hasMore).toBe(false);
        expect(last.groups[0].items.map((p) => p.id)).toEqual(["openai"]);
        expect(last.groups[1].items.map((p) => p.id)).toEqual(["anthropic"]);
    });
    it("renders search first, filtered selected theme marker, and no redundant palette UI", () => {
        const html = renderToStaticMarkup(
            <ThemePicker
                selected="pastelbeans"
                query="pastelbeans"
                search={noop}
                choose={noop}
                back={noop}
            />,
        );
        expect(html.indexOf("<input")).toBeLessThan(html.indexOf("<button"));
        expect(html).toContain('aria-label="Search themes"');
        expect(html).toContain('aria-pressed="true"');
        expect(html).toContain('aria-label="Selected"');
        expect(html).toContain("Back to Appearance");
        expect(html).not.toMatch(
            /102|palettes|Find a theme|<select|<dialog|composer-panel/,
        );
        const empty = renderToStaticMarkup(
            <ThemePicker
                selected="pastelbeans"
                query="no-such-theme"
                search={noop}
                choose={noop}
                back={noop}
            />,
        );
        expect(empty).toContain("No matching themes.");
        expect(empty).not.toContain('aria-pressed="true"');
    });
});
