import { createMemo, createSignal, Show, type ComponentProps } from "solid-js";
import { ADD_PROVIDER_VALUE, modelPickerRootOptions, modelProviderOptions, providerModelOptions } from "../agent/model-picker";
import { InlinePicker } from "./InlinePicker";

/** Provider and model screens share the same input-anchored native card. */
export function SelectionPicker(props: ComponentProps<typeof InlinePicker> & { onAddProvider: () => void }) {
  const [provider, setProvider] = createSignal<string>();
  const [lastProvider, setLastProvider] = createSignal<string>();
  const providers = createMemo(() => modelProviderOptions(props.options || []));
  const rootOptions = createMemo(() => modelPickerRootOptions(props.options || []));
  const models = createMemo(() => providerModelOptions(props.options || [], provider() || ""));
  return <Show when={props.kind === "model"} fallback={<InlinePicker {...props} />}>
    <Show when={provider() || "providers"} keyed>{screen => {
      const detail = screen !== "providers";
      return <InlinePicker {...props}
        title={detail ? providers().find(row => row.value === screen)?.title || screen : "Select model"}
        placeholder={detail ? "Search models" : "Search recent models or providers"}
        options={detail ? models() : rootOptions()}
        current={detail ? props.current : lastProvider() || props.current}
        providerRows={!detail}
        onBack={detail ? () => setProvider(undefined) : undefined}
        onSelect={value => {
          if (value === ADD_PROVIDER_VALUE) props.onAddProvider();
          else if (detail || !value || value.includes("/")) props.onSelect?.(value);
          else { setLastProvider(value); setProvider(value); }
        }} />;
    }}</Show>
  </Show>;
}