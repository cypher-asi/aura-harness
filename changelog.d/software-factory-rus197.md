### The software factory survives a restart

##### Fixed

- A channel member that was planning, building or shipping when the app
  closed carries on at the same stage when it reopens. It plans again in
  plan mode, builds its plan, or is asked to ship, and its report then
  reaches the coordinator. Before, it lost its place: it could leave its
  plan unbuilt or never ship, and the report it was holding back never
  arrived.
