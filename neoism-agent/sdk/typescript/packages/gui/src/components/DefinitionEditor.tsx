import type { NeoismClient } from "@neoism/sdk";
import type { Editor } from "../management";
import { SkillDefinitionEditor } from "./SkillDefinitionEditor";
import { WorkflowDefinitionEditor } from "./WorkflowDefinitionEditor";

/** Preserve the full typed definition; child forms patch fields, never rebuild bundles. */
export function DefinitionEditor({ editor, change, client, directory, errors, onError }: {
    editor: Editor; change(editor: Editor): void; client?: NeoismClient; directory?: string;
    errors?: Record<string, string>; onError?(message: string): void;
}) {
    if (editor.kind === "skills") return <SkillDefinitionEditor editor={editor} change={change} errors={errors} />;
    return <WorkflowDefinitionEditor value={editor.definition} onChange={definition => change({ ...editor, definition })} editing={editor.existing} disabled={editor.readOnly} client={client} directory={directory} onError={onError} />;
}
