import { useEffect, useId, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { Check, ChevronDown } from "lucide-react";
import type { Version } from "../api";

export function MinecraftVersionBrowser({ versions, value, onSelect, open, setOpen, button }: {
  versions: Version[];
  value: string;
  onSelect: (id: string) => void;
  open: boolean;
  setOpen: (open: boolean) => void;
  button: RefObject<HTMLButtonElement | null>;
}) {
  return <SearchableSelect label="Minecraft version" options={versions.filter(({ type }) => type === "release").map(({ id }) => ({ id }))}
    value={value} onSelect={onSelect} open={open} setOpen={setOpen} button={button} placeholder="Find a release" />;
}

export function SearchableSelect({
  label, options, value, onSelect, open, setOpen, button, placeholder, disabled, empty = "No matching versions.", allowCustom, searchable = true,
}: {
  label: string;
  options: { id: string; label?: string }[];
  value: string;
  onSelect: (id: string) => void;
  open: boolean;
  setOpen: (open: boolean) => void;
  button: RefObject<HTMLButtonElement | null>;
  placeholder: string;
  disabled?: boolean;
  empty?: string;
  allowCustom?: boolean;
  searchable?: boolean;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const panelId = useId();
  const search = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);

  const term = query.trim().toLowerCase();
  const matches = open ? options.filter((option) => (option.label ?? option.id).toLowerCase().includes(term)) : [];
  if (open && allowCustom && /^[a-zA-Z0-9][a-zA-Z0-9._+-]{0,39}$/.test(query.trim()) && !options.some(({ id }) => id === query.trim())) {
    matches.push({ id: query.trim() });
  }
  const current = matches[active];

  useEffect(() => {
    if (open) (searchable ? search.current : list.current)?.focus();
  }, [open, searchable]);

  function resetResults() {
    setActive(0);
    if (list.current) list.current.scrollTop = 0;
  }

  function select(id: string) {
    onSelect(id);
    setOpen(false);
    button.current?.focus();
  }

  function handleListKey(event: KeyboardEvent<HTMLUListElement>) {
    const next = {
      ArrowUp: Math.max(0, active - 1),
      ArrowDown: Math.min(matches.length - 1, active + 1),
      Home: 0,
      End: matches.length - 1,
    }[event.key];

    if (next !== undefined && matches.length > 0) {
      event.preventDefault();
      setActive(next);
      list.current?.children[next]?.scrollIntoView({ block: "nearest" });
    } else if (current && (event.key === "Enter" || event.key === " ")) {
      event.preventDefault();
      select(current.id);
    }
  }

  return <div className="version-picker" onBlur={(event) => {
    if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  }} onKeyDown={(event) => {
    if (open && event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      button.current?.focus();
    }
  }}>
    <label id={`${panelId}-label`} htmlFor={`${panelId}-button`}>{label}</label>
    <button ref={button} id={`${panelId}-button`} className="version-picker-trigger" type="button" disabled={disabled}
      aria-expanded={open} aria-controls={panelId} aria-labelledby={`${panelId}-label ${panelId}-value`}
      onClick={() => {
        if (!open) {
          setQuery("");
          resetResults();
        }
        setOpen(!open);
      }}>
      <span id={`${panelId}-value`}>{options.find(({ id }) => id === value)?.label ?? (value || "Select a version")}</span><ChevronDown size={15} aria-hidden="true" />
    </button>
    <div id={panelId} className="version-picker-panel" hidden={!open}>
      {open && <>
          {searchable && <input ref={search} type="search" aria-label={"Search " + label.toLowerCase()} placeholder={placeholder} value={query}
            maxLength={96} onChange={(event) => { setQuery(event.target.value); resetResults(); }}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown" && current) {
                event.preventDefault();
                list.current?.focus();
              } else if (event.key === "Enter") {
                event.preventDefault();
                if (current) select(current.id);
              }
            }} />}
        {current ? <ul ref={list} className="version-list" role="listbox" tabIndex={0} aria-label={label}
          aria-activedescendant={`${panelId}-option-${active}`} onKeyDown={handleListKey}>
          {matches.map(({ id, label: optionLabel }, index) => <li key={id} id={`${panelId}-option-${index}`} className="version-option" role="option"
            aria-selected={value === id} data-active={active === index} onMouseDown={(event) => event.preventDefault()} onClick={() => select(id)}>
            <span>{optionLabel ?? id}</span>{value === id && <Check size={13} aria-hidden="true" />}
          </li>)}
        </ul> : <p className="version-picker-empty field-hint" role="status">{empty}</p>}
      </>}
    </div>
  </div>;
}
