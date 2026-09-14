import { readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// TypeScript resolves .ts before .tsx. Case-insensitive hosts can therefore
// select a different module even when the two full filenames are distinct.
export function checkModulePaths(paths) {
    const names = new Map();
    for (const path of paths) {
        const key = path.replace(/\.(?:tsx?|jsx?|mts|cts)$/i, "").toLowerCase();
        const previous = names.get(key);
        if (previous !== undefined) {
            throw new Error(`Non-portable module paths: ${previous} and ${path}`);
        }
        names.set(key, path);
    }
}

function sourcePaths(root, directory = root) {
    return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
        const path = join(directory, entry.name);
        return entry.isDirectory() ? sourcePaths(root, path)
            : /\.(?:tsx?|jsx?|mts|cts)$/i.test(entry.name) ? [relative(root, path)] : [];
    });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    checkModulePaths(sourcePaths(resolve(dirname(fileURLToPath(import.meta.url)), "../src")));
    console.log("GUI module paths are case-insensitive filesystem safe");
}
