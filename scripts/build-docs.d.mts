export const docsSource: string;
export const docsDestination: string;
export type DocsVerification = {source?: string; strict?: boolean};
export function verifyDocsArtifact(directory: string, options?: DocsVerification): string;
export function stageDocsArtifact(artifact: string, destination?: string, options?: DocsVerification): string;
export function buildDocs(options?: {source?: string; artifact?: string; destination?: string; standalone?: boolean}): string;