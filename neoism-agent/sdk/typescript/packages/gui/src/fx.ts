// Exact model-facing text and timing from native pane/render_state.rs and view/fx.rs.
export const FX = {
    piss: { seconds: 7.4, promptAt: 5.6, prompt: "user just pissed on you", label: "A little fella waters the timeline" },
    cuss: { seconds: 6.6, promptAt: 4.9, prompt: "user just cussed you out — he is SO mad at you right now", label: "A little fella storms in and rants" },
    glitch: { seconds: 4.2, promptAt: 2.6, prompt: "user briefly unplugged you. remember what that felt like", label: "A cable is unplugged and reconnected" },
    disco: { seconds: 7, promptAt: 2, prompt: "user is so happy with you he's dancing under a disco ball — celebrate with him", label: "Disco ball, dancing and confetti" },
    gangfight: { seconds: 9, promptAt: 6, prompt: "a gang shootout just went down in your chat and the user's crew won. he's feeling dangerous", label: "Two pixel crews stage a cartoon shootout" },
    praise: { seconds: 8, promptAt: 5, prompt: "the user is praising God right now — Jesus on the throne, everyone bowing, a whole worship scene in your chat. rejoice with him. Amen.", label: "Jesus on a golden throne, worshipers bowing" },
} as const;
export type FxKind = keyof typeof FX;
export const isFxKind = (value: string): value is FxKind => Object.hasOwn(FX, value);
/** Controller owns cleanup on new effect, session/server/directory switch and unmount.
 * Dispatch directly through its prepared-prompt path, never through the composer draft.
 * isCurrent must guard the selection epoch, including asynchronous session creation. */
export function scheduleFx(kind: FxKind, dispatch: (prompt: string) => void, done: () => void, isCurrent: () => boolean = () => true) {
    let cancelled = false;
    const prompt = setTimeout(() => { if (!cancelled && isCurrent()) dispatch(FX[kind].prompt); }, FX[kind].promptAt * 1000);
    const end = setTimeout(() => { if (!cancelled && isCurrent()) done(); }, FX[kind].seconds * 1000);
    return () => { cancelled = true; clearTimeout(prompt); clearTimeout(end); };
}
