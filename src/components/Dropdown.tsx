import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { CSSProperties, KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown } from 'lucide-react';

type Option<T> = readonly [value: T, label: string];

export function Dropdown<T extends string | number>({
  label, value, options, onChange, placeholder = 'Select…', className = '',
}: {
  label: string;
  value: T;
  options: ReadonlyArray<Option<T>>;
  onChange: (value: T) => void;
  placeholder?: string;
  className?: string;
}) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLUListElement>(null);
  const search = useRef({ prefix: '', time: 0 });
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [position, setPosition] = useState<CSSProperties>({});
  const selected = options.findIndex(([key]) => key === value);

  const choose = (index: number) => {
    const option = options[index];
    if (option && option[0] !== value) onChange(option[0]);
    setOpen(false);
    trigger.current?.focus();
  };

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = trigger.current?.getBoundingClientRect();
      if (!rect) return;
      const inset = 8;
      const gap = 5;
      const below = window.innerHeight - rect.bottom - inset - gap;
      const above = rect.top - inset - gap;
      const upwards = below < Math.min(options.length * 34 + 12, 240) && above > below;
      const width = Math.min(window.innerWidth - 2 * inset, Math.max(148, rect.width));
      setPosition({
        left: Math.max(inset, Math.min(rect.left, window.innerWidth - width - inset)),
        width,
        maxHeight: Math.max(0, Math.min(256, upwards ? above : below)),
        ...(upwards ? { bottom: window.innerHeight - rect.top + gap } : { top: rect.bottom + gap }),
      });
    };
    place();
    // Scroll events can fire many times a frame; place once per frame.
    let frame = 0;
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(place); };
    window.addEventListener('resize', schedule);
    window.addEventListener('scroll', schedule, true);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener('resize', schedule);
      window.removeEventListener('scroll', schedule, true);
    };
  }, [open, options.length]);

  useEffect(() => {
    if (!open) return;
    const dismiss = (event: Event) => {
      if (event.target instanceof Node && !trigger.current?.contains(event.target) &&
          !menu.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener('pointerdown', dismiss);
    document.addEventListener('focusin', dismiss);
    return () => {
      document.removeEventListener('pointerdown', dismiss);
      document.removeEventListener('focusin', dismiss);
    };
  }, [open]);

  useLayoutEffect(() => {
    if (open) menu.current?.children[active]?.scrollIntoView?.({ block: 'nearest' });
  }, [active, open]);

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const { key } = event;
    const last = options.length - 1;
    if (key === 'Escape' && open) {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
    } else if (key === 'Tab') {
      setOpen(false);
    } else if (key === 'ArrowDown' || key === 'ArrowUp' || key === 'Home' || key === 'End') {
      event.preventDefault();
      const step = key === 'ArrowDown' ? 1 : -1;
      const start = selected < 0 ? (step === 1 ? 0 : last) : selected;
      setActive(key === 'Home' ? 0 : key === 'End' ? last
        : open ? Math.min(last, Math.max(0, active + step)) : start);
      setOpen(true);
    } else if (open && (key === 'Enter' || key === ' ')) {
      event.preventDefault();
      choose(active);
    } else if (key.length === 1 && key !== ' ' && !event.metaKey && !event.ctrlKey && !event.altKey) {
      const now = Date.now();
      const previous = now - search.current.time < 650 ? search.current.prefix : '';
      const prefix = previous + key.toLocaleLowerCase();
      search.current = { prefix, time: now };
      const match = options.findIndex(([, text]) => text.toLocaleLowerCase().startsWith(prefix));
      if (match >= 0) {
        event.preventDefault();
        setActive(match);
        setOpen(true);
      }
    }
  };

  return (
    <div className={`dropdown ${className}`}>
      <button
        ref={trigger}
        type="button"
        className="button dropdown-trigger"
        role="combobox"
        aria-label={label}
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        aria-activedescendant={open && options[active] ? `${id}-${active}` : undefined}
        disabled={options.length === 0}
        onClick={() => {
          setActive(Math.max(0, selected));
          search.current = { prefix: '', time: 0 };
          setOpen(!open);
        }}
        onKeyDown={onKeyDown}
      >
        <span>{options[selected]?.[1] ?? placeholder}</span>
        <ChevronDown size={14} aria-hidden="true" />
      </button>
      {open && createPortal(
        <ul ref={menu} id={id} role="listbox" aria-label={label} className="dropdown-menu" style={position}>
          {options.map(([key, text], index) => (
            <li
              key={key}
              id={`${id}-${index}`}
              role="option"
              aria-selected={key === value}
              className={index === active ? 'is-highlighted' : undefined}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => choose(index)}
            >
              <span>{text}</span>
              {key === value && <Check size={14} aria-hidden="true" />}
            </li>
          ))}
        </ul>,
        document.body,
      )}
    </div>
  );
}
