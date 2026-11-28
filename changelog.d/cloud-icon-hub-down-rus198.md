### Cloud agents show their cloud while the cloud is down

##### Fixed

- A cloud agent shows its `☁` on PROJECTS, its channel's MEMBERS and its
  subagents' rows from the first frame, whether or not the cloud can be
  reached. Before, when the cloud was down at start-up, no cloud agent
  showed one for the whole run. The app reads where its agents were
  placed from what it saved, and the cloud, once it is up, says the same.
  An agent it didn't place stays local.
