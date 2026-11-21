import * as Menu from "@radix-ui/react-dropdown-menu";
import { ArrowDownAZ, ArrowUpZA, Check, ChevronDown, ChevronRight, ClockArrowDown, ClockArrowUp, ListFilter, Plus, X } from "lucide-react";
import { modSides, type ModSide } from "../api";
import { Button } from "./ui/button";

export type ContentKind = "all" | "mods" | "resourcePacks" | "shaders";
export type ContentSort = "az" | "za" | "newest" | "oldest";
type ModState = "disabled" | "enabled";
type FilterFlag = "updates" | "openSource" | "external";

export type InstalledFilters = {
  environments: ModSide[];
  states: ModState[];
  updates: boolean;
  openSource: boolean;
  external: boolean;
};

export const emptyFilters: InstalledFilters = { environments: [], states: [], updates: false, openSource: false, external: false };

const contentNames: Record<ContentKind, string> = {
  all: "All", mods: "Mods", resourcePacks: "Resource Packs", shaders: "Shaders",
};
const stateNames: Record<ModState, string> = { disabled: "Disabled", enabled: "Enabled" };
const filterNames: Record<FilterFlag, string> = { updates: "Update available", openSource: "Open source", external: "External" };
const sortOptions = {
  az: { label: "Name (A–Z)", Icon: ArrowDownAZ },
  za: { label: "Name (Z–A)", Icon: ArrowUpZA },
  newest: { label: "Newest first", Icon: ClockArrowDown },
  oldest: { label: "Oldest first", Icon: ClockArrowUp },
};

export function ContentSortAndTypes({ kind, sort, onKind, onSort }: {
  kind: ContentKind; sort: ContentSort; onKind: (kind: ContentKind) => void; onSort: (sort: ContentSort) => void;
}) {
  const { Icon, label } = sortOptions[sort];
  return <div className="actions-wrap">
    <Menu.Root modal={false}>
      <Menu.Trigger asChild><Button size="small" aria-label={"Sort content: " + label}><Icon /> {label} <ChevronDown /></Button></Menu.Trigger>
      <Menu.Portal>
        <Menu.Content className="content-menu" align="start" sideOffset={6} collisionPadding={12} aria-label="Sort content">
          <Menu.RadioGroup value={sort} onValueChange={(value) => onSort(value as ContentSort)}>
            {(Object.keys(sortOptions) as ContentSort[]).map((value) => {
              const option = sortOptions[value];
              return <Menu.RadioItem key={value} value={value} className="content-menu-item">
                <option.Icon size={16} /> {option.label}<Menu.ItemIndicator className="content-menu-marker"><Check size={15} /></Menu.ItemIndicator>
              </Menu.RadioItem>;
            })}
          </Menu.RadioGroup>
        </Menu.Content>
      </Menu.Portal>
    </Menu.Root>
    <div className="content-kind-switch" aria-label="Installed content type">
      <ListFilter size={16} aria-hidden="true" className="content-type-icon" />
      {(Object.keys(contentNames) as ContentKind[]).map((value) => <button type="button" key={value} aria-pressed={kind === value}
        onClick={() => onKind(value)}>{contentNames[value]}</button>)}
    </div>
  </div>;
}

export function ContentFilters({ value, onChange }: { value: InstalledFilters; onChange: (filters: InstalledFilters) => void }) {
  const flags = Object.keys(filterNames) as FilterFlag[];
  const chips = [
    ...value.environments.map((side) => ({ label: modSides[side], remove: () => onChange({ ...value, environments: value.environments.filter((item) => item !== side) }) })),
    ...value.states.map((state) => ({ label: stateNames[state], remove: () => onChange({ ...value, states: value.states.filter((item) => item !== state) }) })),
    ...flags.filter((flag) => value[flag]).map((flag) => ({ label: filterNames[flag], remove: () => onChange({ ...value, [flag]: false }) })),
  ];

  return <div className="content-filter-row actions-wrap">
    <Menu.Root modal={false}>
      <Menu.Trigger asChild><Button size="small" className="content-filter-trigger"><Plus /> Filter</Button></Menu.Trigger>
      <Menu.Portal>
        <Menu.Content className="content-menu content-filter-menu" align="start" sideOffset={6} collisionPadding={12} aria-label="Filter installed mods">
          <FilterGroup label="Environment" choices={modSides} selected={value.environments} onChange={(environments) => onChange({ ...value, environments })} />
          <FilterGroup label="State" choices={stateNames} selected={value.states} onChange={(states) => onChange({ ...value, states })} />
          <Menu.Separator className="content-menu-divider" />
          {flags.map((flag) => <Menu.CheckboxItem key={flag} className="content-menu-item" checked={value[flag]}
            onSelect={(event) => event.preventDefault()} onCheckedChange={(checked) => onChange({ ...value, [flag]: checked })}>
            {filterNames[flag]}<Menu.ItemIndicator className="content-menu-marker"><Check size={15} /></Menu.ItemIndicator>
          </Menu.CheckboxItem>)}
        </Menu.Content>
      </Menu.Portal>
    </Menu.Root>
    {chips.map(({ label, remove }) => <button type="button" className="content-filter-chip" key={label} onClick={remove}
      aria-label={"Remove filter: " + label}>{label}<X size={13} /></button>)}
    {chips.length > 1 && <Button size="small" variant="ghost" onClick={() => onChange(emptyFilters)}>Clear all</Button>}
  </div>;
}

function FilterGroup<T extends string>({ label, choices, selected, onChange }: {
  label: string; choices: Record<T, string>; selected: T[]; onChange: (values: T[]) => void;
}) {
  return <Menu.Sub>
    <Menu.SubTrigger className="content-menu-item">{label}
      {selected.length > 0 && <span className="content-menu-count">{selected.length}</span>}
      <ChevronRight className="content-menu-marker" size={15} />
    </Menu.SubTrigger>
    <Menu.Portal>
      <Menu.SubContent className="content-menu content-filter-submenu" sideOffset={6} alignOffset={-5} collisionPadding={12} aria-label={label}>
        <div className="content-menu-summary">
          <Menu.Label>{selected.length} selected</Menu.Label>
          <Menu.Item className="content-menu-item" disabled={!selected.length} onSelect={(event) => {
            event.preventDefault();
            onChange([]);
          }}>Clear</Menu.Item>
        </div>
        {(Object.keys(choices) as T[]).map((choice) => <Menu.CheckboxItem key={choice} className="content-menu-item"
          checked={selected.includes(choice)} onSelect={(event) => event.preventDefault()}
          onCheckedChange={(checked) => onChange(checked ? [...selected, choice] : selected.filter((value) => value !== choice))}>
          {choices[choice]}<Menu.ItemIndicator className="content-menu-marker"><Check size={15} /></Menu.ItemIndicator>
        </Menu.CheckboxItem>)}
      </Menu.SubContent>
    </Menu.Portal>
  </Menu.Sub>;
}
