import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

/** CSS owns layout motion; mounted scrollers retain their offsets through reversals. */
export function PanelShell({ open, className, id, returnFocus, lazy = false, children }: {
    open: boolean; className: string; id?: string; returnFocus: string; lazy?: boolean; children: ReactNode;
}) {
    const root = useRef<HTMLDivElement>(null);
    const [visited, setVisited] = useState(open);
    if (open && !visited) setVisited(true);
    useLayoutEffect(() => {
        if (!open && root.current?.contains(document.activeElement)) {
            root.current.closest(".app")?.querySelector<HTMLButtonElement>(returnFocus)?.focus({ preventScroll: true });
        }
    }, [open, returnFocus]);
    return <div ref={root} id={id} className={className} data-open={open} inert={!open} aria-hidden={!open}>
        {(!lazy || open || visited) && children}
    </div>;
}

export function useMobileNavigation() {
    const [mobile, setMobile] = useState(() => typeof window !== "undefined" && window.matchMedia("(max-width: 640px)").matches);
    useLayoutEffect(() => {
        const media = window.matchMedia("(max-width: 640px)");
        const update = () => setMobile(media.matches);
        update();
        media.addEventListener("change", update);
        return () => media.removeEventListener("change", update);
    }, []);
    return mobile;
}
