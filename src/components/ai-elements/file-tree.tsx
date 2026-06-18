"use client"

import type { HTMLAttributes, ReactNode } from "react"

import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { cn } from "@/lib/utils"
import {
  BracesIcon,
  ChevronRightIcon,
  ContainerIcon,
  DatabaseIcon,
  FileCodeIcon,
  FileCogIcon,
  FileIcon,
  FileSpreadsheetIcon,
  FileTextIcon,
  FolderIcon,
  FolderOpenIcon,
  PackageIcon,
  SparklesIcon,
  SquareTerminalIcon,
} from "lucide-react"
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
} from "react"

interface FileTreeContextType {
  expandedPaths: Set<string>
  togglePath: (path: string) => void
  selectedPath?: string
  onSelect?: (path: string) => void
}

// Default noop for context default value
// oxlint-disable-next-line eslint(no-empty-function)
const noop = () => {}

const FileTreeContext = createContext<FileTreeContextType>({
  // oxlint-disable-next-line eslint-plugin-unicorn(no-new-builtin)
  expandedPaths: new Set(),
  togglePath: noop,
})

export type FileTreeProps = Omit<HTMLAttributes<HTMLDivElement>, "onSelect"> & {
  expanded?: Set<string>
  defaultExpanded?: Set<string>
  selectedPath?: string
  onSelect?: (path: string) => void
  onExpandedChange?: (expanded: Set<string>) => void
}

export const FileTree = ({
  expanded: controlledExpanded,
  defaultExpanded = new Set(),
  selectedPath,
  onSelect,
  onExpandedChange,
  className,
  children,
  ...props
}: FileTreeProps) => {
  const [internalExpanded, setInternalExpanded] = useState(defaultExpanded)
  const expandedPaths = controlledExpanded ?? internalExpanded

  const togglePath = useCallback(
    (path: string) => {
      const newExpanded = new Set(expandedPaths)
      if (newExpanded.has(path)) {
        newExpanded.delete(path)
      } else {
        newExpanded.add(path)
      }
      setInternalExpanded(newExpanded)
      onExpandedChange?.(newExpanded)
    },
    [expandedPaths, onExpandedChange]
  )

  const contextValue = useMemo(
    () => ({ expandedPaths, onSelect, selectedPath, togglePath }),
    [expandedPaths, onSelect, selectedPath, togglePath]
  )

  return (
    <FileTreeContext.Provider value={contextValue}>
      <div
        className={cn(
          "bg-transparent text-[13px] leading-6 text-foreground",
          className
        )}
        role="tree"
        {...props}
      >
        <div className="w-max min-w-full">{children}</div>
      </div>
    </FileTreeContext.Provider>
  )
}

interface FileTreeFolderContextType {
  path: string
  name: string
  isExpanded: boolean
}

const FileTreeFolderContext = createContext<FileTreeFolderContextType>({
  isExpanded: false,
  name: "",
  path: "",
})

export type FileTreeFolderProps = HTMLAttributes<HTMLDivElement> & {
  path: string
  name: string
  nameClassName?: string
  iconClassName?: string
  showIcon?: boolean
  suffix?: ReactNode
  suffixClassName?: string
}

export const FileTreeFolder = ({
  path,
  name,
  nameClassName,
  iconClassName,
  showIcon = true,
  suffix,
  suffixClassName,
  className,
  children,
  ...props
}: FileTreeFolderProps) => {
  const { expandedPaths, togglePath, selectedPath, onSelect } =
    useContext(FileTreeContext)
  const isExpanded = expandedPaths.has(path)
  const isSelected = selectedPath === path

  const handleOpenChange = useCallback(() => {
    togglePath(path)
  }, [togglePath, path])

  const handleSelect = useCallback(() => {
    onSelect?.(path)
  }, [onSelect, path])

  const folderContextValue = useMemo(
    () => ({ isExpanded, name, path }),
    [isExpanded, name, path]
  )

  return (
    <FileTreeFolderContext.Provider value={folderContextValue}>
      <Collapsible onOpenChange={handleOpenChange} open={isExpanded}>
        <div
          className={cn("", className)}
          aria-selected={isSelected}
          role="treeitem"
          tabIndex={0}
          {...props}
        >
          <CollapsibleTrigger asChild>
            <button
              className={cn(
                "flex h-6 w-max min-w-full items-center gap-1 rounded-md px-1.5 text-left text-foreground transition-colors hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring/45",
                isSelected && "bg-accent/80 text-foreground"
              )}
              onClick={handleSelect}
              type="button"
            >
              <ChevronRightIcon
                className={cn(
                  "size-3.5 shrink-0 text-muted-foreground/70 transition-transform",
                  isExpanded && "rotate-90"
                )}
              />
              {showIcon ? (
                <FileTreeIcon data-testid="file-tree-folder-icon">
                  {isExpanded ? (
                    <FolderOpenIcon
                      className={cn("size-4 text-blue-500", iconClassName)}
                    />
                  ) : (
                    <FolderIcon
                      className={cn("size-4 text-blue-500", iconClassName)}
                    />
                  )}
                </FileTreeIcon>
              ) : null}
              <FileTreeName className={nameClassName}>{name}</FileTreeName>
              {suffix ? (
                <span
                  className={cn(
                    "ml-1 shrink-0 whitespace-nowrap text-muted-foreground/60",
                    suffixClassName
                  )}
                >
                  {suffix}
                </span>
              ) : null}
            </button>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div
              className="ml-3 border-l border-border/40 pl-2"
              data-testid="file-tree-folder-children"
            >
              {children}
            </div>
          </CollapsibleContent>
        </div>
      </Collapsible>
    </FileTreeFolderContext.Provider>
  )
}

interface FileTreeFileContextType {
  path: string
  name: string
}

const FileTreeFileContext = createContext<FileTreeFileContextType>({
  name: "",
  path: "",
})

function getFileExtension(name: string): string {
  const lowerName = name.toLowerCase()
  if (lowerName.endsWith(".d.ts")) return "d.ts"
  const dotIndex = lowerName.lastIndexOf(".")
  return dotIndex >= 0 ? lowerName.slice(dotIndex + 1) : ""
}

type FileTreeBadgeIconProps = {
  label: string
  type: string
  className: string
}

function FileTreeBadgeIcon({ label, type, className }: FileTreeBadgeIconProps) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-flex size-4 items-center justify-center rounded-sm text-[9px] font-semibold leading-none",
        className
      )}
      data-file-icon={type}
    >
      {label}
    </span>
  )
}

export function getFileTreeFileIcon(name: string): ReactNode {
  const lowerName = name.toLowerCase()
  const extension = getFileExtension(name)

  if (
    lowerName === "dockerfile" ||
    lowerName.startsWith("dockerfile.") ||
    lowerName === "docker-compose.yml" ||
    lowerName === "docker-compose.yaml"
  ) {
    return (
      <ContainerIcon className="size-4 text-sky-400" data-file-icon="docker" />
    )
  }

  if (lowerName === "claude.md") {
    return (
      <SparklesIcon
        className="size-4 text-orange-400"
        data-file-icon="claude"
      />
    )
  }

  if (
    lowerName === "package.json" ||
    lowerName === "package-lock.json" ||
    lowerName === "pnpm-lock.yaml" ||
    lowerName === "pnpm-lock.yml" ||
    lowerName === "yarn.lock" ||
    lowerName === "bun.lock" ||
    lowerName === "bun.lockb"
  ) {
    return (
      <PackageIcon
        className="size-4 text-orange-400"
        data-file-icon="package"
      />
    )
  }

  if (extension === "ts" || extension === "tsx" || extension === "d.ts") {
    return (
      <FileTreeBadgeIcon
        className="bg-blue-950/40 text-blue-400"
        label="TS"
        type="typescript"
      />
    )
  }

  if (extension === "js" || extension === "jsx") {
    return (
      <FileTreeBadgeIcon
        className="bg-yellow-950/40 text-yellow-400"
        label="JS"
        type="javascript"
      />
    )
  }

  if (extension === "json") {
    return (
      <BracesIcon className="size-4 text-orange-400" data-file-icon="json" />
    )
  }

  if (extension === "md" || extension === "mdx") {
    return (
      <FileTreeBadgeIcon
        className="bg-emerald-950/40 text-emerald-400"
        label="M"
        type="markdown"
      />
    )
  }

  if (extension === "sh" || extension === "bash" || extension === "zsh") {
    return (
      <SquareTerminalIcon
        className="size-4 text-green-500"
        data-file-icon="shell"
      />
    )
  }

  if (extension === "ps1") {
    return (
      <SquareTerminalIcon
        className="size-4 text-blue-500"
        data-file-icon="powershell"
      />
    )
  }

  if (extension === "yml" || extension === "yaml") {
    return <BracesIcon className="size-4 text-sky-400" data-file-icon="yaml" />
  }

  if (extension === "sql") {
    return (
      <DatabaseIcon className="size-4 text-fuchsia-400" data-file-icon="sql" />
    )
  }

  if (["csv", "tsv", "xls", "xlsx"].includes(extension)) {
    return (
      <FileSpreadsheetIcon
        className="size-4 text-cyan-400"
        data-file-icon="spreadsheet"
      />
    )
  }

  if (
    lowerName.includes("config") ||
    lowerName.startsWith(".") ||
    lowerName === "components.json"
  ) {
    return (
      <FileCogIcon className="size-4 text-violet-400" data-file-icon="config" />
    )
  }

  if (lowerName === "license" || extension === "txt") {
    return (
      <FileTextIcon
        className="size-4 text-muted-foreground/80"
        data-file-icon="text"
      />
    )
  }

  if (
    ["rs", "go", "py", "java", "kt", "swift", "c", "cpp", "h"].includes(
      extension
    )
  ) {
    return (
      <FileCodeIcon className="size-4 text-blue-400" data-file-icon="code" />
    )
  }

  return (
    <FileIcon
      className="size-4 text-muted-foreground/70"
      data-file-icon="file"
    />
  )
}

export type FileTreeFileProps = HTMLAttributes<HTMLDivElement> & {
  path: string
  name: string
  icon?: ReactNode
  nameClassName?: string
  prefix?: ReactNode
  suffix?: ReactNode
  suffixClassName?: string
}

export const FileTreeFile = ({
  path,
  name,
  icon,
  nameClassName,
  prefix,
  suffix,
  suffixClassName,
  className,
  children,
  ...props
}: FileTreeFileProps) => {
  const { selectedPath, onSelect } = useContext(FileTreeContext)
  const isSelected = selectedPath === path

  const handleClick = useCallback(() => {
    onSelect?.(path)
  }, [onSelect, path])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        onSelect?.(path)
      }
    },
    [onSelect, path]
  )

  const fileContextValue = useMemo(() => ({ name, path }), [name, path])

  return (
    <FileTreeFileContext.Provider value={fileContextValue}>
      <div
        className={cn(
          "flex h-6 w-max min-w-full cursor-pointer items-center gap-1.5 rounded-md px-1.5 text-foreground transition-colors hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring/45",
          isSelected && "bg-accent/80 text-foreground",
          className
        )}
        onClick={handleClick}
        onKeyDown={handleKeyDown}
        aria-selected={isSelected}
        role="treeitem"
        tabIndex={0}
        {...props}
      >
        {children ?? (
          <>
            {prefix ?? <span className="size-3.5" />}
            <FileTreeIcon>{icon ?? getFileTreeFileIcon(name)}</FileTreeIcon>
            <FileTreeName className={nameClassName}>{name}</FileTreeName>
            {suffix ? (
              <span
                className={cn(
                  "ml-auto shrink-0 whitespace-nowrap pl-3 text-[11px] tabular-nums text-muted-foreground/70",
                  suffixClassName
                )}
              >
                {suffix}
              </span>
            ) : null}
          </>
        )}
      </div>
    </FileTreeFileContext.Provider>
  )
}

export type FileTreeIconProps = HTMLAttributes<HTMLSpanElement>

export const FileTreeIcon = ({
  className,
  children,
  ...props
}: FileTreeIconProps) => (
  <span className={cn("shrink-0", className)} {...props}>
    {children}
  </span>
)

export type FileTreeNameProps = HTMLAttributes<HTMLSpanElement>

export const FileTreeName = ({
  className,
  children,
  ...props
}: FileTreeNameProps) => (
  <span
    className={cn(
      "shrink-0 whitespace-nowrap leading-6 text-foreground",
      className
    )}
    {...props}
  >
    {children}
  </span>
)

export type FileTreeActionsProps = HTMLAttributes<HTMLDivElement>

const stopPropagation = (e: React.SyntheticEvent) => e.stopPropagation()

export const FileTreeActions = ({
  className,
  children,
  ...props
}: FileTreeActionsProps) => (
  // biome-ignore lint/a11y/noNoninteractiveElementInteractions: stopPropagation required for nested interactions
  // biome-ignore lint/a11y/useSemanticElements: fieldset doesn't fit this UI pattern
  <div
    className={cn("ml-auto flex items-center gap-1", className)}
    onClick={stopPropagation}
    onKeyDown={stopPropagation}
    role="group"
    {...props}
  >
    {children}
  </div>
)
