import {
  useState, useMemo, useCallback, useRef, type ReactNode,
} from "react";
import {
  useReactTable, getCoreRowModel, getFilteredRowModel,
  getSortedRowModel, flexRender, type ColumnDef, type SortingState, type RowSelectionState,
  type ColumnResizeMode, type PaginationState,
} from "@tanstack/react-table";
import {
  IconChevronLeft, IconChevronRight, IconDots, IconSearch, IconArrowUp, IconArrowDown,
} from "@tabler/icons-react";
import { cn } from "../lib/utils";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../primitives/table";
import { Button } from "../primitives/button";
import { Input } from "../primitives/input";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from "../primitives/dropdown-menu";

interface RowAction<T> {
  label: string;
  icon?: ReactNode;
  onClick: (row: T) => void;
  destructive?: boolean;
}

interface BulkAction<T> {
  label: string;
  icon?: ReactNode;
  onClick: (rows: T[]) => void;
  destructive?: boolean;
}

interface DataTableProps<T> {
  data: T[];
  columns: ColumnDef<T, unknown>[];
  loading?: boolean;
  searchable?: boolean;
  searchPlaceholder?: string;
  pageSize?: number;
  rowCount?: number;
  onPaginationChange?: (pagination: PaginationState) => void;
  onSortingChange?: (sorting: SortingState) => void;
  selectable?: boolean;
  resizable?: boolean;
  rowActions?: RowAction<T>[];
  bulkActions?: BulkAction<T>[];
  emptyState?: ReactNode;
  onRowClick?: (row: T) => void;
  className?: string;
}

function SkeletonRows({ columns }: { columns: number }) {
  const widths = useMemo(
    () => Array.from({ length: columns }, () => `${40 + Math.random() * 50}%`),
    [columns],
  );
  return (
    <>
      {Array.from({ length: 5 }, (_, i) => (
        <TableRow key={i}>
          {widths.map((w, j) => (
            <TableCell key={j}>
              <div
                className="h-3.5 animate-pulse rounded bg-muted"
                style={{ width: w, animationDelay: `${i * 75}ms` }}
              />
            </TableCell>
          ))}
        </TableRow>
      ))}
    </>
  );
}

function ResizeHandle({ header }: { header: { getResizeHandler: () => (e: unknown) => void; column: { getIsResizing: () => boolean } } }) {
  return (
    <div
      onMouseDown={header.getResizeHandler()}
      className={cn(
        "absolute right-0 top-0 hidden h-full w-1 cursor-col-resize select-none opacity-0 hover:opacity-100 group-hover/head:opacity-50 md:block",
        header.column.getIsResizing() && "opacity-100 bg-primary",
      )}
    />
  );
}

export function DataTable<T extends { id: string }>({
  data,
  columns: userColumns,
  loading = false,
  searchable = false,
  searchPlaceholder = "Search...",
  pageSize = 10,
  rowCount,
  onPaginationChange,
  onSortingChange: onSortingChangeProp,
  selectable = false,
  resizable = false,
  rowActions,
  bulkActions,
  emptyState,
  onRowClick,
  className,
}: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>([]);
  const [globalFilter, setGlobalFilter] = useState("");
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
  const [pagination, setPagination] = useState<PaginationState>({ pageIndex: 0, pageSize });
  const [focusedIndex, setFocusedIndex] = useState(-1);
  const containerRef = useRef<HTMLDivElement>(null);

  const manualPagination = rowCount != null && onPaginationChange != null;
  const manualSorting = onSortingChangeProp != null;

  const handleSortingChange = useCallback(
    (updater: SortingState | ((prev: SortingState) => SortingState)) => {
      const next = typeof updater === "function" ? updater(sorting) : updater;
      setSorting(next);
      onSortingChangeProp?.(next);
    },
    [sorting, onSortingChangeProp],
  );

  const handlePaginationChange = useCallback(
    (updater: PaginationState | ((prev: PaginationState) => PaginationState)) => {
      const next = typeof updater === "function" ? updater(pagination) : updater;
      setPagination(next);
      onPaginationChange?.(next);
    },
    [pagination, onPaginationChange],
  );

  const columns = useMemo<ColumnDef<T, unknown>[]>(() => {
    const cols: ColumnDef<T, unknown>[] = [];

    if (selectable) {
      cols.push({
        id: "_select",
        header: ({ table }) => (
          <label className="flex h-8 w-8 cursor-pointer items-center justify-center">
            <input
              type="checkbox"
              className="h-4 w-4 rounded border-input"
              checked={table.getIsAllPageRowsSelected()}
              onChange={(e) => table.toggleAllPageRowsSelected(e.target.checked)}
            />
          </label>
        ),
        cell: ({ row }) => (
          <label className="flex h-8 w-8 cursor-pointer items-center justify-center" onClick={(e) => e.stopPropagation()}>
            <input
              type="checkbox"
              className="h-4 w-4 rounded border-input"
              checked={row.getIsSelected()}
              onChange={(e) => row.toggleSelected(e.target.checked)}
            />
          </label>
        ),
        size: 40,
        enableSorting: false,
        enableResizing: false,
      });
    }

    cols.push(...userColumns);

    if (rowActions?.length) {
      cols.push({
        id: "_actions",
        header: () => null,
        cell: ({ row }) => (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" className="h-7 w-7" onClick={(e) => e.stopPropagation()}>
                <IconDots className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {rowActions.map((action) => (
                <DropdownMenuItem
                  key={action.label}
                  onClick={(e) => { e.stopPropagation(); action.onClick(row.original); }}
                  className={action.destructive ? "text-destructive focus:text-destructive" : undefined}
                >
                  {action.icon}
                  {action.label}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        ),
        size: 40,
        enableSorting: false,
        enableResizing: false,
      });
    }

    return cols;
  }, [userColumns, selectable, rowActions]);

  const columnResizeMode: ColumnResizeMode = "onChange";

  const table = useReactTable({
    data,
    columns,
    state: { sorting, globalFilter, rowSelection, pagination },
    onSortingChange: handleSortingChange,
    onGlobalFilterChange: setGlobalFilter,
    onRowSelectionChange: setRowSelection,
    onPaginationChange: handlePaginationChange,
    getCoreRowModel: getCoreRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    ...(manualSorting ? { manualSorting: true } : { getSortedRowModel: getSortedRowModel() }),
    manualPagination,
    ...(manualPagination ? { rowCount } : {}),
    ...(resizable ? { columnResizeMode, enableColumnResizing: true } : {}),
    getRowId: (row) => row.id,
  });

  const rows = table.getRowModel().rows;
  const selectedRows = table.getFilteredSelectedRowModel().rows.map((r) => r.original);
  const pageCount = table.getPageCount();

  const onKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (!rows.length) return;
    const last = rows.length - 1;
    let next = focusedIndex;

    switch (e.key) {
      case "ArrowDown": e.preventDefault(); next = focusedIndex < last ? focusedIndex + 1 : 0; break;
      case "ArrowUp": e.preventDefault(); next = focusedIndex > 0 ? focusedIndex - 1 : last; break;
      case "Home": e.preventDefault(); next = 0; break;
      case "End": e.preventDefault(); next = last; break;
      case "Enter":
        if (focusedIndex >= 0 && onRowClick) onRowClick(rows[focusedIndex].original);
        return;
      default: return;
    }

    setFocusedIndex(next);
    containerRef.current?.querySelectorAll<HTMLElement>("tbody tr")[next]?.scrollIntoView({ block: "nearest" });
  }, [rows, focusedIndex, onRowClick]);

  return (
    <div className={cn("flex flex-col gap-3", className)}>
      {(searchable || (bulkActions && selectedRows.length > 0)) && (
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          {searchable && (
            <div className="relative w-full sm:max-w-sm">
              <IconSearch className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
              <Input
                placeholder={searchPlaceholder}
                value={globalFilter}
                onChange={(e) => setGlobalFilter(e.target.value)}
                className="pl-8"
              />
            </div>
          )}
          {bulkActions && selectedRows.length > 0 && (
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-sm text-muted-foreground">{selectedRows.length} selected</span>
              {bulkActions.map((action) => (
                <Button
                  key={action.label}
                  variant={action.destructive ? "destructive" : "secondary"}
                  size="sm"
                  onClick={() => action.onClick(selectedRows)}
                >
                  {action.icon}
                  {action.label}
                </Button>
              ))}
            </div>
          )}
        </div>
      )}

      <div
        ref={containerRef}
        className="min-h-0 flex-1 overflow-auto rounded-md border outline-none"
        tabIndex={0}
        onKeyDown={onKeyDown}
        onBlur={() => setFocusedIndex(-1)}
      >
        <Table style={resizable ? { width: table.getCenterTotalSize() } : undefined}>
          <TableHeader>
            {table.getHeaderGroups().map((headerGroup) => (
              <TableRow key={headerGroup.id} className="hover:bg-transparent">
                {headerGroup.headers.map((header) => (
                  <TableHead
                    key={header.id}
                    className={cn("group/head", header.column.getCanSort() && "cursor-pointer select-none")}
                    style={{ width: header.getSize() !== 150 ? header.getSize() : undefined }}
                    onClick={header.column.getToggleSortingHandler()}
                  >
                    <div className="flex items-center gap-1">
                      {header.isPlaceholder ? null : flexRender(header.column.columnDef.header, header.getContext())}
                      {header.column.getIsSorted() === "asc" && <IconArrowUp className="h-3 w-3" />}
                      {header.column.getIsSorted() === "desc" && <IconArrowDown className="h-3 w-3" />}
                    </div>
                    {resizable && header.column.getCanResize() && <ResizeHandle header={header} />}
                  </TableHead>
                ))}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {loading ? (
              <SkeletonRows columns={columns.length} />
            ) : rows.length === 0 ? (
              <TableRow>
                <TableCell colSpan={columns.length} className="h-24 text-center">
                  {emptyState || <span className="text-muted-foreground">No results.</span>}
                </TableCell>
              </TableRow>
            ) : (
              rows.map((row, i) => (
                <TableRow
                  key={row.id}
                  data-state={row.getIsSelected() && "selected"}
                  className={cn(
                    onRowClick && "cursor-pointer",
                    i === focusedIndex && "ring-2 ring-inset ring-primary/40",
                  )}
                  onClick={onRowClick ? () => onRowClick(row.original) : undefined}
                  onMouseEnter={() => setFocusedIndex(i)}
                >
                  {row.getVisibleCells().map((cell) => (
                    <TableCell key={cell.id}>
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </TableCell>
                  ))}
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      {pageCount > 1 && !loading && (
        <div className="flex flex-col-reverse items-stretch gap-2 sm:flex-row sm:items-center sm:justify-between">
          <p className="text-center text-sm text-muted-foreground sm:text-left">
            Page {pagination.pageIndex + 1} of {pageCount}
          </p>
          <div className="flex items-center justify-end gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => table.previousPage()}
              disabled={!table.getCanPreviousPage()}
              aria-label="Previous page"
            >
              <IconChevronLeft className="h-4 w-4" />
              <span className="hidden sm:inline">Previous</span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => table.nextPage()}
              disabled={!table.getCanNextPage()}
              aria-label="Next page"
            >
              <span className="hidden sm:inline">Next</span>
              <IconChevronRight className="h-4 w-4" />
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
