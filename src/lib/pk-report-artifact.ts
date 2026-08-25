const STORAGE_COMPATIBILITY_SCRIPT = `<script data-codeg-storage-compat>
(()=>{
  const createMemoryStorage=()=>{
    let values=Object.create(null)
    return {
      get length(){return Object.keys(values).length},
      key(index){return Object.keys(values)[Number(index)]??null},
      getItem(key){key=String(key);return Object.prototype.hasOwnProperty.call(values,key)?values[key]:null},
      setItem(key,value){values[String(key)]=String(value)},
      removeItem(key){delete values[String(key)]},
      clear(){values=Object.create(null)}
    }
  }
  for(const name of ["localStorage","sessionStorage"]){
    try{
      const storage=window[name]
      if(storage&&typeof storage.getItem==="function")continue
    }catch{}
    try{
      Object.defineProperty(window,name,{configurable:true,enumerable:true,value:createMemoryStorage()})
    }catch{}
  }
})()
</script>`

/**
 * Keep standalone contestant HTML runnable inside the report's opaque-origin
 * iframe. Web Storage access throws in that sandbox, so install a document-
 * local in-memory implementation before any contestant script can execute.
 */
export function preparePkReportArtifactHtml(html: string): string {
  if (html.includes("data-codeg-storage-compat")) return html

  const doctype = /^\s*<!doctype[^>]*>/i.exec(html)
  if (!doctype) return `${STORAGE_COMPATIBILITY_SCRIPT}${html}`

  return `${html.slice(0, doctype[0].length)}${STORAGE_COMPATIBILITY_SCRIPT}${html.slice(doctype[0].length)}`
}

export function decodePkReportArtifact(contentBase64: string): string {
  const binary = atob(contentBase64)
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
  return new TextDecoder().decode(bytes)
}

export function encodePkReportArtifact(html: string): string {
  const bytes = new TextEncoder().encode(html)
  const chunks: string[] = []
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    chunks.push(String.fromCharCode(...bytes.subarray(offset, offset + 0x8000)))
  }
  return btoa(chunks.join(""))
}
