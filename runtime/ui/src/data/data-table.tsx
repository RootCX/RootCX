import * as React from "react";
import {
  useReactTable,
  getCoreRowModel,
  getFilteredRowModel,
  getPaginationRowModel,
  getSortedRowModel,
  flexRender,
  type ColumnDef,
  type SortingState,
  type RowSelectionState,
} from "@tanstack/react-table";
import { IconChevronLeft, IconChevronRight, IconDots, IconSearch, IconArrowUp, IconArrowDown } from "@tabler/icons-react";
import { cn } from "../lib/utils";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../primitives/table";
import { Button } from "../primitives/button";
import { Input } from "../primitives/input";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from "../primitives/dropdown-menu";

interface RowAction<T> {
  label: string;
  icon?: React.ReactNode;
  onClick: (row: T) => void;
  destructive?: boolean;
}

interface BulkAction<T> {
  label: string;
  icon?: React.ReactNode;
  onClick: (rows: T[]) => void;
  destructive?: boolean;
}

interface DataTableProps<T> {
  data: T[];
  columns: ColumnDef<T, unknown>[];
  loading?: boolean;
  searchable?: boolean;
  searchPlaceholder?: string;
  pagination?: boolean;
  pageSize?: number;
  selectable?: boolean;
  rowActions?: RowAction<T>[];
  bulkActions?: BulkAction<T>[];
  emptyState?: React.ReactNode;
  onRowClick?: (row: T) => void;
  className?: string;
}

function SkeletonRows({ columns, rows = 5 }: { columns: number; rows?: number }) {
  return (
    <>
      {Array.from({ length: rows }).map((_, i) => (
        <TableRow key={i}>
          {Array.from({ length: columns }).map((_, j) => (
            <TableCell key={j}>
              <div className="h-4 w-full animate-pulse rounded bg-muted" />
            </TableCell>
          ))}
        </TableRow>
      ))}
    </>
  );
}

export function DataTable<T extends { id: string }>({
  data,
  columns: userColumns,
  loading = false,
  searchable = false,
  searchPlaceholder = "Search...",
  pagination = true,
  pageSize = 10,
  selectable = false,
  rowActions,
  bulkActions,
  emptyState,
  onRowClick,
  className,
}: DataTableProps<T>) {
  const [sorting, setSorting] = React.useState<SortingState>([]);
  const [globalFilter, setGlobalFilter] = React.useState("");
  const [rowSelection, setRowSelection] = React.useState<RowSelectionState>({});

  const columns = React.useMemo<ColumnDef<T, unknown>[]>(() => {
    const cols: ColumnDef<T, unknown>[] = [];

    if (selectable) {
      cols.push({
        id: "_select",
        header: ({ table }) => (
          <input
            type="checkbox"
            className="h-4 w-4 rounded border-input"
            checked={table.getIsAllPageRowsSelected()}
            onChange={(e) => table.toggleAllPageRowsSelected(e.target.checked)}
          />
        ),
        cell: ({ row }) => (
          <input
            type="checkbox"
            className="h-4 w-4 rounded border-input"
            checked={row.getIsSelected()}
            onChange={(e) => {
              e.stopPropagation();
              row.toggleSelected(e.target.checked);
            }}
          />
        ),
        size: 40,
        enableSorting: false,
      });
    }

    cols.push(...userColumns);

    if (rowActions && rowActions.length > 0) {
      cols.push({
        id: "_actions",
        header: () => null,
        cell: ({ row }) => (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" className="h-8 w-8" onClick={(e) => e.stopPropagation()}>
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
        size: 48,
        enableSorting: false,
      });
    }

    return cols;
  }, [userColumns, selectable, rowActions]);

  const table = useReactTable({
    data,
    columns,
    state: { sorting, globalFilter, rowSelection },
    onSortingChange: setSorting,
    onGlobalFilterChange: setGlobalFilter,
    onRowSelectionChange: setRowSelection,
    getCoreRowModel: getCoreRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    getSortedRowModel: getSortedRowModel(),
    ...(pagination ? { getPaginationRowModel: getPaginationRowModel() } : {}),
    initialState: { pagination: { pageSize } },
    getRowId: (row) => row.id,
  });

  const selectedRows = table.getFilteredSelectedRowModel().rows.map((r) => r.original);

  return (
    <div className={cn("space-y-4", className)}>
      {(searchable || (bulkActions && selectedRows.length > 0)) && (
        <div className="flex items-center justify-between gap-2">
          {searchable && (
            <div className="relative max-w-sm">
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
            <div className="flex items-center gap-2">
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

      <div className="rounded-md border">
        <Table>
          <TableHeader>
            {table.getHeaderGroups().map((headerGroup) => (
              <TableRow key={headerGroup.id}>
                {headerGroup.headers.map((header) => (
                  <TableHead
                    key={header.id}
                    style={{ width: header.getSize() !== 150 ? header.getSize() : undefined }}
                    className={header.column.getCanSort() ? "cursor-pointer select-none" : undefined}
                    onClick={header.column.getToggleSortingHandler()}
                  >
                    <div className="flex items-center gap-1">
                      {header.isPlaceholder ? null : flexRender(header.column.columnDef.header, header.getContext())}
                      {header.column.getIsSorted() === "asc" && <IconArrowUp className="h-3 w-3" />}
                      {header.column.getIsSorted() === "desc" && <IconArrowDown className="h-3 w-3" />}
                    </div>
                  </TableHead>
                ))}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {loading ? (
              <SkeletonRows columns={columns.length} />
            ) : table.getRowModel().rows.length === 0 ? (
              <TableRow>
                <TableCell colSpan={columns.length} className="h-24 text-center">
                  {emptyState || <span className="text-muted-foreground">No results.</span>}
                </TableCell>
              </TableRow>
            ) : (
              table.getRowModel().rows.map((row) => (
                <TableRow
                  key={row.id}
                  data-state={row.getIsSelected() && "selected"}
                  className={onRowClick ? "cursor-pointer" : undefined}
                  onClick={onRowClick ? () => onRowClick(row.original) : undefined}
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

      {pagination && !loading && data.length > pageSize && (
        <div className="flex items-center justify-between">
          <p className="text-sm text-muted-foreground">
            Page {table.getState().pagination.pageIndex + 1} of {table.getPageCount()}
          </p>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => table.previousPage()}
              disabled={!table.getCanPreviousPage()}
            >
              <IconChevronLeft className="h-4 w-4" />
              Previous
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => table.nextPage()}
              disabled={!table.getCanNextPage()}
            >
              Next
              <IconChevronRight className="h-4 w-4" />
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
