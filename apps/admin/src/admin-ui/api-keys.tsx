import type React from "react";
import { Trans, useLingui } from "@lingui/react/macro";
import { Link, useNavigate, useParams } from "react-router";
import { KeyRoundIcon, PlusIcon, Trash2Icon } from "lucide-react";

import { adminPaths } from "../admin-routes";
import { formatDate } from "./helpers";
import { EmptyNotice, Field, MissingRecordView } from "./shared";
import type { CreateKeyForm } from "./types";
import type { ApiKey } from "../types";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";

export function ApiKeysView({
  apiKeys,
  onCreate,
  onToggle,
}: {
  apiKeys: ApiKey[];
  onCreate: () => void;
  onToggle: (key: ApiKey) => void;
}) {
  const { t } = useLingui();

  return (
    <Card>
      <CardHeader className="flex-row items-start justify-between gap-4">
        <div className="flex flex-col gap-1">
          <CardTitle>
            <Trans>API Keys</Trans>
          </CardTitle>
          <CardDescription>
            <Trans>Client credentials, limits, and service permissions.</Trans>
          </CardDescription>
        </div>
        <Button type="button" onClick={onCreate}>
          <PlusIcon data-icon="inline-start" />
          <Trans>Create</Trans>
        </Button>
      </CardHeader>
      <CardContent>
        <div className="hidden md:block">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>
                  <Trans>Name</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Preview</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Permissions</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Limits</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Last used</Trans>
                </TableHead>
                <TableHead className="text-right">
                  <Trans>Actions</Trans>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {apiKeys.map((key) => (
                <TableRow key={key.id}>
                  <TableCell>
                    <div className="flex flex-col gap-1">
                      <Link
                        to={adminPaths.key(key.id)}
                        className="font-medium underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        {key.name}
                      </Link>
                      <span className="text-xs text-muted-foreground">
                        {key.description || t`No description`}
                      </span>
                    </div>
                  </TableCell>
                  <TableCell className="font-mono text-xs">{key.keyPreview}</TableCell>
                  <TableCell>
                    {key.permissions.length === 0 ? t`All` : key.permissions.join(", ")}
                  </TableCell>
                  <TableCell>
                    <div className="flex flex-col gap-1 text-xs text-muted-foreground">
                      <span>
                        {key.rateLimitPerMinute ? (
                          <Trans>{key.rateLimitPerMinute} rpm</Trans>
                        ) : (
                          <Trans>No rpm</Trans>
                        )}
                      </span>
                      <span>
                        {key.concurrencyLimit ? (
                          <Trans>{key.concurrencyLimit} concurrent</Trans>
                        ) : (
                          <Trans>No concurrent</Trans>
                        )}
                      </span>
                    </div>
                  </TableCell>
                  <TableCell>{formatDate(key.lastUsedAt)}</TableCell>
                  <TableCell className="text-right">
                    <Button type="button" variant="outline" size="sm" onClick={() => onToggle(key)}>
                      {key.isActive ? t`Active` : t`Paused`}
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
              {apiKeys.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={6}>
                    <EmptyNotice
                      title={t`No API keys`}
                      body={t`Create a key before connecting a client.`}
                    />
                  </TableCell>
                </TableRow>
              ) : null}
            </TableBody>
          </Table>
        </div>
        <div className="grid gap-3 md:hidden">
          {apiKeys.length === 0 ? (
            <EmptyNotice
              title={t`No API keys`}
              body={t`Create a key before connecting a client.`}
            />
          ) : (
            apiKeys.map((key) => (
              <div key={key.id} className="flex flex-col gap-3 rounded-md border p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <Link
                      to={adminPaths.key(key.id)}
                      className="block truncate text-sm font-medium underline-offset-4 hover:underline"
                    >
                      {key.name}
                    </Link>
                    <div className="truncate font-mono text-xs text-muted-foreground">
                      {key.keyPreview}
                    </div>
                  </div>
                  <Button type="button" variant="outline" size="sm" onClick={() => onToggle(key)}>
                    {key.isActive ? t`Active` : t`Paused`}
                  </Button>
                </div>
                <div className="grid grid-cols-2 gap-2 text-xs text-muted-foreground">
                  <span>
                    {key.permissions.length === 0 ? t`All services` : key.permissions.join(", ")}
                  </span>
                  <span>{formatDate(key.lastUsedAt)}</span>
                  <span>
                    {key.rateLimitPerMinute ? (
                      <Trans>{key.rateLimitPerMinute} rpm</Trans>
                    ) : (
                      <Trans>No rpm</Trans>
                    )}
                  </span>
                  <span>
                    {key.concurrencyLimit ? (
                      <Trans>{key.concurrencyLimit} concurrent</Trans>
                    ) : (
                      <Trans>No concurrent</Trans>
                    )}
                  </span>
                </div>
              </div>
            ))
          )}
        </div>
      </CardContent>
    </Card>
  );
}

export function ApiKeyDetailView({
  apiKeys,
  onToggle,
  onDelete,
}: {
  apiKeys: readonly ApiKey[];
  onToggle: (key: ApiKey) => Promise<void>;
  onDelete: (key: ApiKey) => Promise<boolean>;
}) {
  const { t } = useLingui();
  const { keyId } = useParams();
  const navigate = useNavigate();
  const key = apiKeys.find((item) => item.id === keyId);

  if (!key) {
    return (
      <MissingRecordView
        title={t`API key not found`}
        body={t`This API key may have been deleted or the link is incomplete.`}
        to={adminPaths.keys()}
        label={t`Back to API keys`}
      />
    );
  }

  const selectedKey = key;

  async function handleDelete() {
    if (await onDelete(selectedKey)) {
      void navigate(adminPaths.keys());
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col justify-between gap-4 sm:flex-row sm:items-start">
        <div className="flex min-w-0 flex-col gap-2">
          <Button asChild type="button" variant="ghost" size="sm" className="w-fit">
            <Link to={adminPaths.keys()}>
              <Trans>API Keys</Trans>
            </Link>
          </Button>
          <div>
            <div className="text-sm text-muted-foreground">
              <Trans>API Key</Trans>
            </div>
            <h1 className="truncate text-2xl font-semibold">{selectedKey.name}</h1>
            <p className="mt-1 font-mono text-sm text-muted-foreground">{selectedKey.keyPreview}</p>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" onClick={() => void onToggle(key)}>
            {selectedKey.isActive ? t`Disable` : t`Enable`}
          </Button>
          <Button type="button" variant="destructive" onClick={() => void handleDelete()}>
            <Trash2Icon data-icon="inline-start" />
            <Trans>Delete</Trans>
          </Button>
        </div>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>
            <Trans>Credential details</Trans>
          </CardTitle>
          <CardDescription>
            <Trans>Stored metadata and request limits for this client credential.</Trans>
          </CardDescription>
        </CardHeader>
        <CardContent>
          <dl className="grid gap-5 text-sm sm:grid-cols-2 xl:grid-cols-3">
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Description</Trans>
              </dt>
              <dd className="mt-1">{selectedKey.description || t`No description`}</dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Permissions</Trans>
              </dt>
              <dd className="mt-1">
                {selectedKey.permissions.length === 0
                  ? t`All services`
                  : selectedKey.permissions.join(", ")}
              </dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Status</Trans>
              </dt>
              <dd className="mt-1">{selectedKey.isActive ? t`Active` : t`Paused`}</dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Rate limit</Trans>
              </dt>
              <dd className="mt-1">
                {selectedKey.rateLimitPerMinute ? (
                  <Trans>{selectedKey.rateLimitPerMinute} rpm</Trans>
                ) : (
                  <Trans>No rpm</Trans>
                )}
              </dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Concurrency limit</Trans>
              </dt>
              <dd className="mt-1">
                {selectedKey.concurrencyLimit ? (
                  <Trans>{selectedKey.concurrencyLimit} concurrent requests</Trans>
                ) : (
                  <Trans>No concurrent requests</Trans>
                )}
              </dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Daily cost limit</Trans>
              </dt>
              <dd className="mt-1">
                {selectedKey.dailyCostLimit ? `$${selectedKey.dailyCostLimit}` : t`No limit`}
              </dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Created</Trans>
              </dt>
              <dd className="mt-1">{formatDate(selectedKey.createdAt)}</dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Last used</Trans>
              </dt>
              <dd className="mt-1">{formatDate(selectedKey.lastUsedAt)}</dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">
                <Trans>Expires</Trans>
              </dt>
              <dd className="mt-1">{formatDate(selectedKey.expiresAt)}</dd>
            </div>
          </dl>
        </CardContent>
      </Card>
    </div>
  );
}

export function CreateKeySheet({
  open,
  form,
  setForm,
  onOpenChange,
  onSubmit,
}: {
  open: boolean;
  form: CreateKeyForm;
  setForm: React.Dispatch<React.SetStateAction<CreateKeyForm>>;
  onOpenChange: (open: boolean) => void;
  onSubmit: (event: React.FormEvent<HTMLFormElement>) => void;
}) {
  const { t } = useLingui();

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="sm:max-w-md">
        <SheetHeader>
          <SheetTitle>
            <Trans>Create API key</Trans>
          </SheetTitle>
          <SheetDescription>
            <Trans>Issue a client key with optional routing limits.</Trans>
          </SheetDescription>
        </SheetHeader>
        <form className="flex flex-col gap-4 px-4" onSubmit={onSubmit}>
          <Field label={t`Name`} htmlFor="key-name">
            <Input
              id="key-name"
              value={form.name}
              onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))}
              required
            />
          </Field>
          <Field label={t`Description`} htmlFor="key-description">
            <Textarea
              id="key-description"
              value={form.description}
              onChange={(event) =>
                setForm((current) => ({ ...current, description: event.target.value }))
              }
            />
          </Field>
          <Field label={t`Permissions`} htmlFor="key-permissions">
            <Select
              value={form.permissions}
              onValueChange={(value) => setForm((current) => ({ ...current, permissions: value }))}
            >
              <SelectTrigger id="key-permissions">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="all">
                    <Trans>All services</Trans>
                  </SelectItem>
                  <SelectItem value="claude">
                    <Trans>Claude only</Trans>
                  </SelectItem>
                  <SelectItem value="openai">
                    <Trans>OpenAI-compatible only</Trans>
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </Field>
          <div className="grid gap-3 sm:grid-cols-3">
            <Field label={t`RPM`} htmlFor="key-rpm">
              <Input
                id="key-rpm"
                inputMode="numeric"
                value={form.rateLimitPerMinute}
                onChange={(event) =>
                  setForm((current) => ({ ...current, rateLimitPerMinute: event.target.value }))
                }
              />
            </Field>
            <Field label={t`Concurrency`} htmlFor="key-concurrency">
              <Input
                id="key-concurrency"
                inputMode="numeric"
                value={form.concurrencyLimit}
                onChange={(event) =>
                  setForm((current) => ({ ...current, concurrencyLimit: event.target.value }))
                }
              />
            </Field>
            <Field label={t`Daily USD`} htmlFor="key-cost">
              <Input
                id="key-cost"
                inputMode="decimal"
                value={form.dailyCostLimit}
                onChange={(event) =>
                  setForm((current) => ({ ...current, dailyCostLimit: event.target.value }))
                }
              />
            </Field>
          </div>
          <SheetFooter>
            <Button type="submit">
              <KeyRoundIcon data-icon="inline-start" />
              <Trans>Create key</Trans>
            </Button>
          </SheetFooter>
        </form>
      </SheetContent>
    </Sheet>
  );
}
