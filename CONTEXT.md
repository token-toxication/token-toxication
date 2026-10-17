# Admin UI domain context

The admin UI manages durable records. Collections are routed to their own URLs,
and each record has a stable detail URL so a copied browser address retains its
meaning after a refresh.

## Provider Account

A Provider Account stores the provider connection and its operational state.
It owns authentication configuration, priority, activation, and provider health.

## Catalog Model

A Catalog Model is a public model identifier exposed by the relay. It describes
the model independently from any provider that can serve it. A Catalog Model is
either manually managed by an operator or managed from one or more provider
catalog sources.

## Managed Catalog Source

A Managed Catalog Source is an authoritative model observation published by a
provider account. It establishes which Catalog Models that account currently
offers; the provider type is a property of the source, not of the Catalog
Model.

## Provider Model Route

A Provider Model Route connects one Catalog Model to one Provider Account and
an upstream model identifier. It carries wire-protocol and route-role settings.

## API Key

An API Key is a durable client credential with permissions and limits. Its
secret is transient creation output and is never represented by a detail URL.

## Request Log

A Request Log is an immutable record of a relay attempt. Its detail screen is
read-only and remains addressable by its identifier.
