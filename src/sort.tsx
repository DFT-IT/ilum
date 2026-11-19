import { currentLocale, t } from "./language";
import { useState } from "react";
import { ArrowDown, ArrowUp, ArrowUpDown } from "lucide-react";

type Direction = "ascending" | "descending";
type Value = string | number | null | undefined;
export type SortState<Key extends string> = { key: Key; direction: Direction; select: (key: Key, first?: Direction) => void };

const collators = {
  "de-DE": new Intl.Collator("de-DE", { numeric: true, sensitivity: "base" }),
  "en-GB": new Intl.Collator("en-GB", { numeric: true, sensitivity: "base" }),
};

export function useSort<Key extends string>(key: Key, direction: Direction) {
  const [sort, setSort] = useState({ key, direction });
  return {
    ...sort,
    select: (next: Key, first: Direction = "ascending") => setSort(current => ({
      key: next,
      direction: current.key === next ? current.direction === "ascending" ? "descending" : "ascending" : first,
    })),
  };
}

export function sortRows<Row, Key extends string>(rows: readonly Row[], sort: SortState<Key>, get: (row: Row, key: Key) => Value) {
  return [...rows].sort((a, b) => {
    const left = get(a, sort.key);
    const right = get(b, sort.key);
    if (left == null) return right == null ? 0 : 1;
    if (right == null) return -1;
    const comparison = typeof left === "number" && typeof right === "number" ? left - right : collators[currentLocale()].compare(String(left), String(right));
    return sort.direction === "ascending" ? comparison : -comparison;
  });
}

export function SortHeader<Key extends string>({ column, sort, label, first }: {
  column: Key; sort: SortState<Key>; label: string; first?: Direction;
}) {
  const active = sort.key === column;
  const Icon = !active ? ArrowUpDown : sort.direction === "ascending" ? ArrowUp : ArrowDown;
  return <th scope="col" aria-sort={active ? sort.direction : undefined}>
    <button className="sort-header" title={t("Nach „{0}“ sortieren", label)} onClick={() => sort.select(column, first)}>
      {label}<Icon size={13} className={active ? "sort-active" : undefined} aria-hidden="true" />
    </button>
  </th>;
}
