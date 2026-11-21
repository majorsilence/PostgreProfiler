import { shallowRef } from 'vue'
import { api } from '../api'
import type { Names } from './trace'

interface NamesResponse {
  roles: { oid: number | string; name: string }[]
  databases: { oid: number | string; name: string }[]
}

/** Role and database names by OID (the trace only carries OIDs). Loaded once. */
export const names = shallowRef<Names>({ roles: new Map(), databases: new Map() })
let loaded: Promise<void> | null = null

export function loadNames(): Promise<void> {
  loaded ??= api
    .get<NamesResponse>('/api/names')
    .then((r) => {
      names.value = {
        roles: new Map(r.roles.map((x) => [String(x.oid), x.name])),
        databases: new Map(r.databases.map((x) => [String(x.oid), x.name])),
      }
    })
    .catch(() => {
      loaded = null // try again next time; OIDs are shown meanwhile
    })
  return loaded
}
