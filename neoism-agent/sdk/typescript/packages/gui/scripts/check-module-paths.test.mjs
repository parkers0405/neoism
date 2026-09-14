import assert from "node:assert/strict";
import test from "node:test";
import { checkModulePaths } from "./check-module-paths.mjs";

test("rejects the Windows AttachmentPreview resolution collision", () => {
    assert.throws(() => checkModulePaths([
        "components/AttachmentPreview.tsx", "components/attachmentPreview.ts",
    ]), /Non-portable module paths/);
});

test("rejects case-only paths and extension-shadowed modules", () => {
    for (const paths of [["Foo.ts", "foo.ts"], ["Foo.ts", "Foo.tsx"], ["UI/Foo.ts", "ui/foo.ts"]]) {
        assert.throws(() => checkModulePaths(paths), /Non-portable module paths/);
    }
});

test("allows distinct component and helper module names", () => {
    checkModulePaths(["components/AttachmentPreview.tsx", "components/attachmentPreviewUtils.ts", "other/AttachmentPreview.tsx"]);
});
