import type { HostWorkspaceTreeSummary, WorkspaceSummary } from "../workspace/types";
import { neoismWordmarkSvg } from "./neoismWordmarkSvg";

export interface ConnectionFormValues {
  url: string;
  authToken: string;
}

export interface ConnectionScreenOptions {
  mount: HTMLElement;
  defaultUrl: string;
  onSubmit: (values: ConnectionFormValues) => void;
  onWorkspacePick?: (workspace: WorkspaceSummary) => void;
  onCreateWorkspace?: () => void;
}

/**
 * Initial pre-connect screen: form with a daemon URL, auth token, and a
 * connect button. The host swaps it out for a `TerminalPanel` once the
 * daemon answers with `PtyCreated`.
 */
export class ConnectionScreen {
  private readonly root: HTMLElement;
  private readonly statusEl: HTMLParagraphElement;
  private readonly urlInput: HTMLInputElement;
  private readonly tokenInput: HTMLInputElement;
  private readonly submitButton: HTMLButtonElement;
  private readonly workspaceField: HTMLLabelElement;
  private readonly workspaceSelect: HTMLSelectElement;
  private readonly workspaceActions: HTMLDivElement;
  private workspaces: WorkspaceSummary[] = [];
  private readonly newWorkspaceValue = "__neoism_new_workspace__";

  constructor(private readonly options: ConnectionScreenOptions) {
    this.root = document.createElement("section");
    this.root.className = "connection-screen";

    const form = document.createElement("form");
    form.className = "connection-form";
    form.autocomplete = "off";

    const hero = document.createElement("div");
    hero.className = "connection-hero";
    const wordmark = document.createElement("div");
    wordmark.className = "connection-wordmark";
    wordmark.setAttribute("aria-label", "Neoism");
    wordmark.innerHTML = neoismWordmarkSvg;
    this.installWordmarkHover(wordmark);
    hero.appendChild(wordmark);

    form.appendChild(hero);

    this.urlInput = makeField(form, {
      id: "daemon-url",
      label: "Daemon URL",
      placeholder: options.defaultUrl,
      value: options.defaultUrl,
      type: "text",
      inputMode: "url",
    });

    this.tokenInput = makeField(form, {
      id: "auth-token",
      label: "Auth token",
      placeholder: "(optional)",
      value: "",
      type: "password",
      inputMode: "text",
    });

    this.submitButton = document.createElement("button");
    this.submitButton.type = "submit";
    this.submitButton.className = "connection-submit";
    this.submitButton.textContent = "Connect";
    form.appendChild(this.submitButton);

    this.workspaceField = document.createElement("label");
    this.workspaceField.className = "connection-field connection-workspace-field";
    this.workspaceField.htmlFor = "workspace-select";
    this.workspaceField.hidden = true;
    const workspaceLabel = document.createElement("span");
    workspaceLabel.className = "connection-field-label";
    workspaceLabel.textContent = "Workspace";
    this.workspaceField.appendChild(workspaceLabel);
    this.workspaceSelect = document.createElement("select");
    this.workspaceSelect.id = "workspace-select";
    this.workspaceSelect.name = "workspace-select";
    this.workspaceSelect.className = "connection-workspace-select";
    this.workspaceSelect.disabled = true;
    this.workspaceField.appendChild(this.workspaceSelect);
    form.appendChild(this.workspaceField);

    this.workspaceActions = document.createElement("div");
    this.workspaceActions.className = "connection-workspace-actions";
    this.workspaceActions.hidden = true;
    form.appendChild(this.workspaceActions);

    this.statusEl = document.createElement("p");
    this.statusEl.className = "connection-status";
    this.statusEl.setAttribute("role", "status");
    this.statusEl.setAttribute("aria-live", "polite");
    form.appendChild(this.statusEl);

    form.addEventListener("submit", (event) => this.handleSubmit(event));

    this.root.appendChild(form);
    options.mount.appendChild(this.root);
  }

  setStatus(text: string): void {
    this.statusEl.textContent = text;
    if (/\b(error|failed|rejected|closed)\b/i.test(text)) {
      this.statusEl.dataset.state = "error";
    } else {
      delete this.statusEl.dataset.state;
    }
  }

  setBusy(busy: boolean): void {
    this.submitButton.disabled = busy;
    this.urlInput.disabled = busy || !this.workspaceField.hidden;
    this.tokenInput.disabled = busy || !this.workspaceField.hidden;
    this.workspaceSelect.disabled = busy || (this.workspaces.length === 0 && !this.workspaceSelect.value);
  }

  showWorkspaces(tree: HostWorkspaceTreeSummary | null): void {
    this.urlInput.disabled = true;
    this.tokenInput.disabled = true;
    this.workspaceField.hidden = false;
    this.workspaceActions.hidden = false;
    this.workspaceActions.appendChild(this.submitButton);
    this.workspaceSelect.replaceChildren();

    this.workspaces = [...(tree?.workspaces ?? [])].sort((a, b) => {
      const lastActiveDelta = (b.last_active ?? 0) - (a.last_active ?? 0);
      if (lastActiveDelta !== 0) return lastActiveDelta;
      return a.title.localeCompare(b.title);
    });

    if (this.workspaces.length === 0) {
      const option = document.createElement("option");
      option.value = tree ? this.newWorkspaceValue : "";
      option.textContent = tree ? "+ New workspace..." : "Loading workspaces...";
      this.workspaceSelect.appendChild(option);
      this.workspaceSelect.disabled = !tree;
      this.submitButton.textContent = "Open workspace";
      this.submitButton.disabled = !tree;
      this.statusEl.textContent = tree
        ? "Create a workspace to continue."
        : "Loading workspaces...";
      return;
    }

    for (const workspace of this.workspaces) {
      const option = document.createElement("option");
      option.value = workspace.id;
      option.textContent = `${workspace.title || workspace.id} - ${workspace.root_dir || workspace.id}`;
      this.workspaceSelect.appendChild(option);
    }

    const createOption = document.createElement("option");
    createOption.value = this.newWorkspaceValue;
    createOption.textContent = "+ New workspace...";
    this.workspaceSelect.appendChild(createOption);

    this.workspaceSelect.disabled = false;
    this.submitButton.hidden = false;
    this.submitButton.disabled = false;
    this.submitButton.textContent = "Open workspace";
    this.statusEl.textContent = "";
  }

  dispose(): void {
    this.root.remove();
  }

  private installWordmarkHover(wordmark: HTMLDivElement): void {
    const svg = wordmark.querySelector<SVGSVGElement>("svg");
    if (!svg) return;
    const clear = () => {
      for (let index = 0; index < 6; index += 1) {
        svg.classList.remove(`neoism-hover-letter-${index}`);
      }
    };
    wordmark.addEventListener("pointerover", (event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      const layer = target.closest(".neoism-wordmark-letter");
      if (!(layer instanceof Element)) return;
      const match = [...layer.classList]
        .map((className) => /^neoism-wordmark-letter-(\d)$/.exec(className))
        .find(Boolean);
      if (!match) return;
      clear();
      svg.classList.add(`neoism-hover-letter-${match[1]}`);
    });
    wordmark.addEventListener("pointerleave", clear);
  }

  private handleSubmit(event: SubmitEvent): void {
    event.preventDefault();
    if (!this.workspaceField.hidden) {
      if (this.workspaceSelect.value === this.newWorkspaceValue) {
        this.options.onCreateWorkspace?.();
        return;
      }
      const workspace = this.workspaces.find(
        (candidate) => candidate.id === this.workspaceSelect.value,
      );
      if (workspace) {
        this.options.onWorkspacePick?.(workspace);
      }
      return;
    }
    const url = this.urlInput.value.trim() || this.options.defaultUrl;
    const authToken = this.tokenInput.value.trim();
    this.options.onSubmit({ url, authToken });
  }
}

interface FieldDescriptor {
  id: string;
  label: string;
  placeholder: string;
  value: string;
  type: "text" | "password";
  inputMode: "text" | "url";
}

function makeField(
  parent: HTMLElement,
  descriptor: FieldDescriptor,
): HTMLInputElement {
  const wrapper = document.createElement("label");
  wrapper.className = "connection-field";
  wrapper.htmlFor = descriptor.id;
  const labelText = document.createElement("span");
  labelText.className = "connection-field-label";
  labelText.textContent = descriptor.label;
  wrapper.appendChild(labelText);

  const input = document.createElement("input");
  input.id = descriptor.id;
  input.name = descriptor.id;
  input.type = descriptor.type;
  input.placeholder = descriptor.placeholder;
  input.value = descriptor.value;
  input.inputMode = descriptor.inputMode;
  input.autocomplete = "off";
  wrapper.appendChild(input);

  parent.appendChild(wrapper);
  return input;
}
