import { useId } from "react";
import { Avatar } from "./Identity";
import { messageAuthor } from "../identity";
import "./user-avatar.css";

export function UserAvatar({ info, localName }: { info: { author?: unknown }; localName: string }) {
    const id = useId();
    const name = messageAuthor(info, localName);
    return <span className="user-message-avatar" tabIndex={0} aria-label={name} aria-describedby={id}>
        <Avatar seed={name} />
        <span id={id} className="user-avatar-tooltip" role="tooltip">{name}</span>
    </span>;
}
