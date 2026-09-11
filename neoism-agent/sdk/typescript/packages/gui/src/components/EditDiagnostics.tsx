import { useState } from "react";
import type { CardPart } from "./toolCardData";
import { readEditDiagnostics, type EditDiagnosticFile } from "./editDiagnosticsData";
import "./edit-diagnostics.css";

export interface EditDiagnosticsProps {
    part: CardPart;
    /** Actual edited paths, including moveTo/sourcePath. Augments input/fileChanges. */
    paths?: string[];
}
const CACHE_HINT = "Cached diagnostics may predate this edit; they are not confirmation of current errors.";
const PAGE_SIZE = 100;

/** Mount directly after the actual diff/code block. Owns no tool status or Frame. */
export function EditDiagnostics({ part, paths }: EditDiagnosticsProps) {
    const files = readEditDiagnostics(part, paths);
    if (!files.length) return null;
    return <DiagnosticRows key={part.id} files={files} />;
}
function DiagnosticRows({ files }: { files: EditDiagnosticFile[] }) {
    const [limit, setLimit] = useState(PAGE_SIZE);
    const total = files.reduce((sum, file) => sum + file.diagnostics.length, 0);
    let remaining = limit;
    return <div className="edit-diagnostics" aria-label="Edit diagnostics">
        {files.map(file => {
            const rows = file.diagnostics.slice(0, remaining);
            remaining -= rows.length;
            if (!rows.length) return null;
            return <div className="edit-diagnostics-file" key={file.path} title={file.cached ? CACHE_HINT : undefined}>
                {files.length > 1 && <div className="edit-diagnostics-path">{file.path}</div>}
                {rows.map(row => <div className="edit-diagnostics-row" data-severity={row.severity} key={row.key}>
                    <span className="edit-diagnostics-location">{row.severity === "error" ? "ERROR" : "WARN"} [{row.line}:{row.column}]</span>{" "}{row.message}
                </div>)}
            </div>;
        })}
        {total > limit && <button type="button" className="edit-diagnostics-more" onClick={() => setLimit(value => value + PAGE_SIZE)}>
            Show {Math.min(PAGE_SIZE, total - limit)} more ({total - limit} remaining)
        </button>}
    </div>;
}
