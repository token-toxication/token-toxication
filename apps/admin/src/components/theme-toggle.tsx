import { MonitorIcon, MoonIcon, SunIcon } from "lucide-react";
import { msg } from "@lingui/core/macro";
import { Trans, useLingui } from "@lingui/react/macro";
import { useTheme } from "next-themes";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

const themeOptions = [
  { value: "light", label: msg`Light`, icon: SunIcon },
  { value: "dark", label: msg`Dark`, icon: MoonIcon },
  { value: "system", label: msg`System`, icon: MonitorIcon },
] as const;

function ThemeToggle() {
  const { i18n, t } = useLingui();
  const { theme, setTheme } = useTheme();
  const activeTheme = themeOptions.find((option) => option.value === theme) ?? themeOptions[2];
  const ActiveThemeIcon = activeTheme.icon;
  const activeThemeLabel = i18n._(activeTheme.label);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="icon"
          aria-label={t`Theme: ${activeThemeLabel}`}
        >
          <ActiveThemeIcon />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>
          <Trans>Appearance</Trans>
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup value={activeTheme.value} onValueChange={setTheme}>
          {themeOptions.map((option) => {
            const Icon = option.icon;

            return (
              <DropdownMenuRadioItem key={option.value} value={option.value}>
                <Icon />
                {i18n._(option.label)}
              </DropdownMenuRadioItem>
            );
          })}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export { ThemeToggle };
