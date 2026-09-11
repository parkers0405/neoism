import { useEffect, useRef, useState } from "react";
import { languageName, MAX_SOURCE, type Span } from "./tokens";
import "./syntax.css";

export function SyntaxCode({ source, language }: { source: string; language: string }) {
    const ref = useRef<HTMLElement>(null);
    const [result, setResult] = useState<{ source: string; language: string; spans: Span[] }>();
    useEffect(() => {
        if (!languageName(language) || source.length > MAX_SOURCE || !source) return;
        const controller = new AbortController();
        let timer: ReturnType<typeof setTimeout> | undefined;
        let started = false;
        const start = () => {
            if (started) return;
            started = true;
            // Coalesce streaming fragments before loading/parsing.
            timer = setTimeout(() => {
                void import("./client").then(({ highlight }) => highlight(source, language, controller.signal)).then(spans => {
                    if (!controller.signal.aborted) setResult({ source, language, spans });
                }).catch(() => { /* Plain escaped React text remains visible. */ });
            }, 80);
        };
        let observer: IntersectionObserver | undefined;
        if (typeof IntersectionObserver !== "undefined" && ref.current) {
            observer = new IntersectionObserver(entries => {
                if (entries.some(entry => entry.isIntersecting)) { observer?.disconnect(); start(); }
            });
            observer.observe(ref.current);
        } else start();
        return () => { observer?.disconnect(); clearTimeout(timer); controller.abort(); };
    }, [source, language]);
    const spans = result?.source === source && result.language === language ? result.spans : [];
    return <code ref={ref} className="neo-syntax">{spans.length ? spans.map(span =>
        <span key={span.start} className={`neo-syn-${span.token}`}>{source.slice(span.start, span.end)}</span>
    ) : source}</code>;
}
