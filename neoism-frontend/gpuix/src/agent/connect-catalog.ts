/** The provider picker needs names and connection state, never model catalogs.
 * Project before serializing across FFI or retaining the picker callback. */
export function connectCatalog(catalog:{all:readonly {id:string;name:string}[];connected:readonly string[]}){
 return {all:catalog.all.map(({id,name})=>({id,name})),connected:[...catalog.connected]};
}
