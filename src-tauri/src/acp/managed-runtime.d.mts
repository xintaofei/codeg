export function packageBin(
  name: string,
  command: string,
  version: string,
  searchPath?: string
): string
export function commandFor(entry: string, args: string[]): [string, string[]]
export function main(argv: string[]): void
