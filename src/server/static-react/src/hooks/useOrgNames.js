import { useState, useEffect } from 'react'
import { orgClient } from '../api/clients/orgClient'

/**
 * Hook that fetches org list and returns a map of org_hash → org_name.
 * Shared across components that need to display org context.
 */
export function useOrgNames() {
  const [orgNames, setOrgNames] = useState({})

  useEffect(() => {
    orgClient.listOrgs().then(res => {
      const data = res.data || res
      const orgs = data.orgs || []
      const map = {}
      for (const org of orgs) map[org.org_hash] = org.org_name
      setOrgNames(map)
    }).catch(() => {})
  }, [])

  return orgNames
}
