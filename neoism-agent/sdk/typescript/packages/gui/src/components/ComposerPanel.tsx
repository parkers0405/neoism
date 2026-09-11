import { useEffect, useRef, useState, type ReactNode } from "react";
import { X } from "lucide-react";
import "./composer-panel.css";

export function ComposerPanel({ title, close, children }: {
    title: string;
    close(): void;
    children: ReactNode;
}) {
    const panel = useRef<HTMLElement>(null);
    const closeRef = useRef(close);
    closeRef.current = close;
    const [height, setHeight] = useState(360);
    useEffect(() => {
        const element = panel.current;
        const anchor = element?.parentElement;
        if (!element || !anchor) return;
        const measure = () => setHeight(Math.max(0, Math.min(420, anchor.getBoundingClientRect().top - 12)));
        measure();
        if (!element.contains(document.activeElement)) {
            element.querySelector<HTMLElement>(".composer-panel-content input, .composer-panel-content select, .composer-panel-content button")?.focus();
        }
        const observer = new ResizeObserver(measure);
        observer.observe(anchor);
        window.addEventListener("resize", measure);
        const outside = (event: PointerEvent) => {
            if (event.target instanceof Node && !element.contains(event.target) && !anchor.contains(event.target)) closeRef.current();
        };
        document.addEventListener("pointerdown", outside);
        return () => {
            observer.disconnect();
            window.removeEventListener("resize", measure);
            document.removeEventListener("pointerdown", outside);
            if (anchor.isConnected && (document.activeElement === document.body || element.contains(document.activeElement))) {
                anchor.querySelector("textarea")?.focus();
            }
        };
    }, []);
    const dismiss = () => {
        panel.current?.parentElement?.querySelector("textarea")?.focus();
        close();
    };
    return <section ref={panel} role="dialog" aria-label={title} className="composer-panel" style={{ maxHeight: height }} onKeyDown={(event) => {
        if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            dismiss();
        }
    }}>
        <header className="composer-panel-heading">
            <strong>{title}</strong>
            <button type="button" aria-label={`Close ${title}`} onClick={dismiss}><X size={15} /></button>
        </header>
        <div className="composer-panel-content">{children}</div>
    </section>;
}
