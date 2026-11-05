/** Wire types for neoism-protocol/src/database.rs. Keep in sync with the Rust protocol. */
/** SQLite values never pass 64-bit integers through a JavaScript number. */
export type DatabaseValue = {kind:"null"} | {kind:"integer";value:string} | {kind:"real";value:number} | {kind:"text";value:string} | {kind:"blob";value:string};
export type DatabaseRecord = Record<string,DatabaseValue>;
export interface DatabaseFilter {column:string;values:DatabaseValue[]}
export interface DatabaseSort {column:string;descending:boolean}
/** Updates/deletes require Browse's exact identity and a full original row.
 * Insert keys omitted from values use SQLite defaults; explicit null stays null.
 * Delete values must be empty. Apply is atomic and capped at 200 changes. */
export interface DatabaseChange {kind:"insert"|"update"|"delete";values:DatabaseRecord;identity?:DatabaseRecord;original?:DatabaseRecord}
export type DatabaseRequest =
  | {Schema:{path:string}}
  | {Browse:{path:string;table:string;limit:number;offset:number;sort?:DatabaseSort;filters:DatabaseFilter[]}}
  | {Values:{path:string;table:string;column:string;filters:DatabaseFilter[]}}
  | {Apply:{path:string;table:string;changes:DatabaseChange[]}};
export interface DatabaseColumn {name:string;data_type:string;not_null:boolean;default_value:string|null;primary_key:number}
export interface DatabaseObject {name:string;kind:"table"|"view";sql:string;columns:DatabaseColumn[];indexes:{name:string;unique:boolean;columns:string[]}[];triggers:{name:string;sql:string}[]}
export type DatabaseReply =
  | {Schema:{objects:DatabaseObject[]}}
  | {Rows:{columns:string[];rows:DatabaseValue[][];identities:(DatabaseRecord|null)[];total:number;editable:boolean}}
  | {Values:{values:{value:DatabaseValue;count:number}[];truncated:boolean}}
  | {Applied:{affected:number}}
  | {Error:{message:string}};