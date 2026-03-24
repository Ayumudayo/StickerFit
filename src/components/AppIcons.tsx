import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & {
  size?: number;
};

function IconBase({
  size = 20,
  strokeWidth = 1.8,
  children,
  ...props
}: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth={strokeWidth}
      width={size}
      height={size}
      aria-hidden="true"
      {...props}
    >
      {children}
    </svg>
  );
}

export function FileBadgeIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
      <path d="M14 3v5h5" />
      <path d="M9 13h6" />
      <path d="M9 17h6" />
      <path d="M9 9h1" />
    </IconBase>
  );
}

export function FolderOpenIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M3 7.5A2.5 2.5 0 0 1 5.5 5H9l2 2h7.5A2.5 2.5 0 0 1 21 9.5V10" />
      <path d="M3.5 10.5h17l-1.7 7.4A2.5 2.5 0 0 1 16.36 20H6.34a2.5 2.5 0 0 1-2.44-2.1z" />
    </IconBase>
  );
}

export function FolderDownIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M3 7.5A2.5 2.5 0 0 1 5.5 5H9l2 2h7.5A2.5 2.5 0 0 1 21 9.5v8A2.5 2.5 0 0 1 18.5 20h-13A2.5 2.5 0 0 1 3 17.5z" />
      <path d="M12 10v6" />
      <path d="m9.5 13.5 2.5 2.5 2.5-2.5" />
    </IconBase>
  );
}

export function FileVideoIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <rect x="3.5" y="5" width="12.5" height="14" rx="2.5" />
      <path d="m16 10 4.5-2v8L16 14z" />
      <path d="M7.5 9h4" />
      <path d="M7.5 12h4" />
    </IconBase>
  );
}

export function SettingsIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06A1.65 1.65 0 0 0 15 19.4a1.65 1.65 0 0 0-1 .6 1.65 1.65 0 0 1-3 0 1.65 1.65 0 0 0-1-.6 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-.6-1 1.65 1.65 0 0 1 0-3 1.65 1.65 0 0 0 .6-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-.6 1.65 1.65 0 0 1 3 0 1.65 1.65 0 0 0 1 .6 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9c0 .38.14.74.4 1a1.65 1.65 0 0 1 0 3c-.26.26-.4.62-.4 1Z" />
    </IconBase>
  );
}

export function RefreshIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M20 11a8 8 0 0 0-14.9-4" />
      <path d="M4 4v4h4" />
      <path d="M4 13a8 8 0 0 0 14.9 4" />
      <path d="M20 20v-4h-4" />
    </IconBase>
  );
}

export function ExpandIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M15 3h6v6" />
      <path d="m21 3-7 7" />
      <path d="M9 21H3v-6" />
      <path d="m3 21 7-7" />
    </IconBase>
  );
}

export function InfoIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 10v5" />
      <path d="M12 7.5h.01" />
    </IconBase>
  );
}

export function ChevronDownIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="m6 9 6 6 6-6" />
    </IconBase>
  );
}

export function PlayIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="m8 5 10 7-10 7z" fill="currentColor" stroke="none" />
    </IconBase>
  );
}

export function PauseIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M9 5h2.5v14H9z" fill="currentColor" stroke="none" />
      <path d="M12.5 5H15v14h-2.5z" fill="currentColor" stroke="none" />
    </IconBase>
  );
}

export function CheckCircleIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <circle cx="12" cy="12" r="9" />
      <path d="m8.5 12 2.3 2.3 4.7-4.8" />
    </IconBase>
  );
}

export function SkipBackIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M6 5v14" />
      <path d="m18 6-8 6 8 6z" fill="currentColor" stroke="none" />
    </IconBase>
  );
}

export function SkipForwardIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M18 5v14" />
      <path d="m6 6 8 6-8 6z" fill="currentColor" stroke="none" />
    </IconBase>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="m6 6 12 12" />
      <path d="m18 6-12 12" />
    </IconBase>
  );
}

export function TrashIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <path d="M3 6h18" />
      <path d="M8 6V4.5A1.5 1.5 0 0 1 9.5 3h5A1.5 1.5 0 0 1 16 4.5V6" />
      <path d="M6.5 6 7.4 19A2 2 0 0 0 9.4 21h5.2a2 2 0 0 0 2-2L17.5 6" />
      <path d="M10 10v7" />
      <path d="M14 10v7" />
    </IconBase>
  );
}

export function GridIcon(props: IconProps) {
  return (
    <IconBase {...props}>
      <rect x="4" y="4" width="6" height="6" rx="1.2" />
      <rect x="14" y="4" width="6" height="6" rx="1.2" />
      <rect x="4" y="14" width="6" height="6" rx="1.2" />
      <rect x="14" y="14" width="6" height="6" rx="1.2" />
    </IconBase>
  );
}
