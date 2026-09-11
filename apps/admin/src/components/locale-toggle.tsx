import { LanguagesIcon } from "lucide-react";
import { Trans, useLingui } from "@lingui/react/macro";

import { activateLocale, locales } from "@/i18n";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

function LocaleToggle() {
  const { i18n, t } = useLingui();
  const activeLocale = locales.find((locale) => locale.value === i18n.locale) ?? locales[0];

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="icon"
          aria-label={t`Language: ${activeLocale.label}`}
        >
          <LanguagesIcon />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>
          <Trans>Language</Trans>
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={activeLocale.value}
          onValueChange={(value) => {
            const nextLocale = locales.find((locale) => locale.value === value);
            if (nextLocale) {
              activateLocale(nextLocale.value);
            }
          }}
        >
          {locales.map((locale) => (
            <DropdownMenuRadioItem key={locale.value} value={locale.value}>
              {locale.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export { LocaleToggle };
