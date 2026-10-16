

# What's Changed?

much of the interface should remain the same, with some elements moving around slightly to accomidate. AKA most of this will be very familiar.
However, there are some fundimental changes that may require some rework / redesign of existing components and expectations. 

Big changes: 

## One iter_engine = one server
Today it's one engine per project; I want to collapse that down to one engine per computer, allowing one engine to manage multiple projects. 
This will require all API calls from engine to server to specify what project they're working on, per call.  
i.e., "get next" workitem needs to calculate the next workitem **for a specific named project** not for an engine tied to that project. 

This will allow one engine to support multiple projects on one server. 

## Tigher sync between Project Graph and Repo Files
Today there is the graph, which is a loose interpretation of the .iter.md files in the repo.   This version will strongly align two sources of metadata, making them (within a few seconds) basically identical.

Node:         File:                 Notes / changes:
project       project.iter.md       new: the project level settings (replaces main.iter.md)
code          code.iter.md          mostly the same: type context, container, component, or connection
test          test.iter.md          previously testgroup
bizreq        bizreq.iter.md        broken apart: now 1 requirement = 1 file/node; default should be to keep in a `reqs/` subfolder
techreq       techreq.iter.md       broken apart: now 1 requirement = 1 file/node; default should be to keep in a `reqs/` subfolder
philosophy    philosophy.iter.md    same 
usecase       usecase.iter.md       same
actor         actor.iter.md         new type of "usecase" container, helping organize who is driving use-cases, and where the actual people/users are interacting with your application
n/a           agentmem.iter.md      only written by engine, not sync'd to project graph


reminders: 
- test nodes / test.iter.md files are just the metadata; the real tests live in the repo as test00.sh files, and run many deterministic tests per file.  The test00.sh output should be standardized going forward, providing basic information like:
`{"name":"my test node's name", "overall_success":true, "normal":{"total":50, "pass":50, "err":0}, "longtail":{...}, "failure":{...}, "id":"uuid"}`
- philosophy is the highest-level requirements document, typically only included once globally (although any number can exist). The agents should use the philosophy document to infer missing requirements, resolve conflicting requirements, etc.  basically it becomes a generalized guide as to what the user wants, in a non-iterated format.  


the above tighter sync between project graph and repo files will require (at least) two new services:
- sync: checks for recently changed files and ensures node == file 
- conformance: checks the file side to ensure files are well structured and correct if/where needed, since they (unlike nodes) can be updated outside the iter_ framework.  I'm expecting iter_data to keep the node side well conformed natively.


## Use Project Graph as a Project Designer - major new use-case
Today, repos must exist with an active engine before a project can be linked.  This means a project must exist first. 
I want to be able to design new projects in the project graph, as a starting point, which can then be pushed to the iter_engine, which  (now being project independent) can create the folder structures and git repos on your behalf.  This makes it easy to start and architect new projects using a graphical designer. 

You'll have to come up with a set of rules on how to translate nodes into files (allow the user or agent to setup the repo).  i.e., rules to produce something like the "Mockup Folder Structure" below. 
  

## Multi-Model Provider
While I don't have any plans for this today, I want to start abstracting components so that we can include multilpe llm providers, such as OpenAI or other OSS models.  This primarily means two things:
- dispatch needs a generic "dispatch_agent(provider, model, context, settings)" function which may direct to provider specific functions behind the scenes, aka "dispatch_agent_claude(model, context, settings)"
- "get_usage" becomes abstracted up a level, since it's not clear to me how we'll collect this for other models.  Today Claude simply provide it per call, so  it can just do what it does today, append to the end of the other dispatch output.  Other models may have other requirements, like API calls, etc., so I want to abstract this away from the main dispatcher.


# Other Stuff



## High Level Technical Architecture

See:
https://docs.google.com/drawings/d/19rNjeOARQn4CyLxxVutEtic1E2Azek-NG-6EfnAuV9A

Please review this diagram and tell me what i'm missing.  I think I have everything, but likely I'm missing at least some items. 


This design is intended to better organize the idea of engines, accounts, and projects.  
We decouple engine from project, and accounts from projects.  
Meaning, you have to start the engine, pointing the engine to the iter_data server.
The iter_data server will help the user connect projects to their engine, and
separately, accounts with their engine.  

The only thing the engine needs on startup should be:
- URL for the iter_data server you want to use
- .env file to load, for 
  - LLM account credentials
  - iter_data connection credentials
  - other useful credentials/secrets for agent access

Everything else should come from the project settings.

Since it's easier to visualize, perhaps we borrow the project_graph interface, to allow users to visually setup their system... aka:
- node: iter_data: server you're connected to, when looking at project_graph; there should be only 1 (for now)
- node: iter_engine: engine servers (plural) that sync with this iter_data, including any server-side credentialing needed
- node: project: an individual coding project, served by this iter_data and can span multiple engines
- node: workitem_type: the type of allowed workitems: queued, complete, paused, inprogress, etc., connected to each new project by default
- node: agent: a named agent (code, test, plan, etc.) containing all settings and prompt text, connecting to workitem_type as a default
- node: agent_tools: collection (name|desc) of prompt/instructions that an agent can look-up on-demand; basically identical to claude skills, but stored at the iter_data level, so available across iter_engines / servers. also includes the "_shared" instructions that are always suffixed to agent prompts on startup.
- node: user: an individual whose credentials are allowed to view each project / own each iter_engine or iter_data
- node: account: an LLM provider (claude/future) account, which can be connected to a project; note that the engine still needs the credentials locally to make this work, the edge between account and project is just the permission side, not the credential; also please move the "switch"/"stop" percent logic to the settings of this edge.
- node: others I may have overlooked?

Each of these nodes/edges can have settings attached, with the goal of having ALL settings represented by one of these nodes or edges.  Maybe make a new "use case" dropdown to see "settings", to show the settings graph.

This will also allow admins to easily connect / disconned edges from nodes.  aka a defined project not connected to an engine will not run.  
To not lose edge configuration /settings when temporarily removing access (say, removing edge between engine and project for an afternoon), please make sure there are non-deletable "deactivated" nodes for every type.   

Also in the UI, increase the maturity of the project graph interface;  edges should have it's two ends be drag/dropped to other nodes, or copy/paste edges from one node to another, for quick replication of settings, etc.  Maybe allow the tagging of edges (aka my_project_default edge between engine and project).  Also both nodes and edges will require an option to "configure" and open a pop-up or lightbox, showing an editable list of settings. 



## .iter.md Files
I want a few minor changes to the .iter.md files, although they should be very familiar to past version.

### Mockup Folder Structure

```
project_name/
  ↳ global/
    ↳ myrepo.project.iter.md
    ↳ usecases/
      ↳ environ.usecase.iter.md
      ↳ environ.test.iter.md
      ↳ employer.actor.iter.md
      ↳ employee.actor.iter.md
      ↳ provider.actor.iter.md
    ↳ requirements/
      ↳ global.techreq.iter.md
      ↳ global.bizreq.iter.md
      ↳ global.philosophy.iter.md
  ↳ src/
    ↳ data/
      ↳ data.context.iter.md
      ↳ data.test.iter.md
      ↳ postgres/
        ↳ dockerfile
        ↳ postgres.code.iter.md
        ↳ postgres01.test.iter.md
        ↳ postgres02.test.iter.md
        ↳ plugins/
          ↳ enforcement.component.iter.md
          ↳ enforcement.test.iter.md
    ↳ webui/
      ↳ webui.context.iter.md
      ↳ webui.test.iter.md
      ↳ tailwind/
        ↳ tailwind.code.iter.md
        ↳ tailwind.test.iter.md
        ↳ index.html
```      


## Marker Files (*.iter.md)
All marker files will be structured as <optional_name.><iter_type>.iter.md
this means the name, if it exists, is delimited by a period.  If a name is omitted, then the file can begin with the iter_type (project, context, test, usecase, etc.) directly.

Thus, defining a container with or without a name are both acceptable:
- my_container_name.code.iter.md
- code.iter.md

Often .iter.md files will exist in isolation (i.e., typically only one code.iter.md per directory) but other types will typically exist together (01.test.iter.md, 02.test.iter.md, etc.)

The agents / users can use whichever style they prefer, understanding that specifically naming each file is a bit safer, as it allows future clustering of .iter.md files without conflict. 

iter_engine and iter_data shouldn't care about the filename; they both operate on the frontmatter "id" as the unique key.  When the project_graph is creating nodes (aka .iter.md files), the iter_engine sync process will default to naming the .iter.md file after the node name itself, stripped of non-filesafe characters, and if there are conflicts, it can either add a sequence (01, 02, 03, etc.) or add the last 12 of the id uuid, set in the project settings (default to sequence).



### Common structure

These data structures are common across all marker files.
Note that when an engine is gathering context for agent launch, it will read the entire .iter.md file on which it's working, then gather the [id, name, desc] and filepath for all "children" nodes. The agent can, at it's discretion, also read any of these children it believes to be relevant.  It will also always include local and global bizreq.iter.md and techreq.iter.md files.

The point: the information below is required, and the conformance engine should always check/correct all files when touched. Empty is OK, missing is not. 

fields:
- id: uuid as unique key
- name: short, readable name
- desc: slightly longer summary, 100 or so words, enough for an agent to understand content (aka to read the entire file); think skill summary.
- creator: name of the user or agent (i.e., agent.code) who created the object.
- teststate: same as today
- children: list of path globs that point to children objects (would be edges on the project graph):
  - codedirs:  paths to required code directories where code is read/modified and locks placed; always recurses down the tree from that point
  - codenodes: paths to other children code.iter.md files that are logical children of this one (to cascade testing)
  - tests: test.iter.md file will place paths to the test00.sh files here; all other objects will place paths to the test.iter.md files
  - reqs:  paths to any requirement files, including i.e., bizreq, techreq, philosophy, or other non-iter requirements; global (attached to project.iter.md) requirements are always added, so you don't need to add every time; frequently will be a directory, not file
- timestamps: a collection of timestamps, such as created, last_modified, last_tested, etc.


sample:
```yaml
id: <uuid>
name: "A short, readable name"
desc: "A slightly longer description, around 200-300 characters that would be a very high-level summary of the object."
creator: 
teststate: inherit
children:
  codedirs:   ["{thisfiledir}/src/**"]
  codenodes:  []
  tests: ["{thisfiledir}/test/**"]
  reqs: ["{thisfiledir}/reqs/*bizreq.iter.md", "{thisfiledir}/reqs/*techreq.iter.md"]
timestamps: {"create":"2026-09-30 14:56:11Z", "last_modified":"2026-09-30 14:56:11Z", "last_tested":"2026-09-30 14:56:11Z"}
```

Beyond this, there will be node-specific data that may be required.  For example, a code.iter.md file will require a "level" since it has 4 levels.

If the project graph sync engine sees the above glob format, it should honor the glob, collect the files at the glob location, and sync those instead.  Note that it will NEVER sync a non-marker file (aka not ending with `.iter.md`) even tho they can be left in the repo as requirement files.   They are 


# What's missing?

The big thing I want to remove / rework is interfaces.  Initially the target was to generate a small number of interfaces to promote re-use.  What this evolved into is simply listing the descrete shape of every call made, which is of no resuse value... aka they became specific calls, not reusable interfaces.

This means iter5 can remove all interfaces from the projects; from code.iter.md files, from graph nodes, from the repos themselves.  

## New paradigm

I would like to create a new paradigm; connections.  This fits neatly into the C4 model, being context, container, component, connection (replacing code, which is too low level for our uses).

A connection becomes: a node representing a TYPE of connection, rather than a specific shape of connection.  It can then be used to show the type of connections, and what services them.

For example:
Container: API Gateway (kong)
Container: AppLogic01
Container: AppLogic02
Container: AppLogic03
Connection: API call over HTTP

The graph would be directional, so:

[API Gateway (kong)] --> [API call over HTTP] -|
                                               |
                                               |
                               [AppLogic01] <--| 
                               [AppLogic02] <--| 
                               [AppLogic03] <--| 

Indicating that the "API Gatway" supplies the "API call over HTTP" which goes on to connect "AppLogic" 1-3 together. 

This means we'll have a far fewer connection nodes (API, MCP, gRPC, maybe a couple more), but they'll be connected far wider. This is another reason why we want the ability to add/remove project graph nodes from the viewer by type; most of the time, we won't want that much noise. But when we do (like looking at a connection / network map) then it will be very useful.