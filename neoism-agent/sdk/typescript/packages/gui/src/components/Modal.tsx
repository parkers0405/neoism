import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
export function Modal({
    title,
    children,
    close,
}: {
    title: string;
    children: ReactNode;
    close(): void;
}) {
    const ref = useRef<HTMLDialogElement>(null);
    useEffect(() => {
        const dialog = ref.current!;
        dialog.showModal();
        return () => dialog.close();
    }, []);
    return (
        <dialog
            ref={ref}
            onCancel={close}
            onClick={(e) => {
                if (e.target === e.currentTarget) close();
            }}
        >
            <section className="modal">
                <header>
                    <h2>{title}</h2>
                    <button aria-label="Close" onClick={close}>
                        <X size={20} />
                    </button>
                </header>
                {children}
            </section>
        </dialog>
    );
}
