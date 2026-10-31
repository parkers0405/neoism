import type {HistoryMessage,Todo} from "../daemon/protocol";

/** Reopened sessions have persisted task lists before any live TodoUpdate.
 * An explicitly empty list clears tasks; it must not resurrect older work. */
export function sessionTodos(history:readonly HistoryMessage[],live?:readonly Todo[]):readonly Todo[]{
 if(live!==undefined)return live;
 for(let index=history.length-1;index>=0;index--){
  const message=history[index]!,todos=message.todos;
  if(todos?.length||message.id==="todos-snapshot"||["todowrite","todo_write","todo"].includes(message.tool?.toLowerCase()||""))return todos||[];
 }
 return [];
}
