cd ~/dev/pdy-dev && ~/dev/iterapp/iter3/bin/iter_engine --config .iter/config.json

https://ivi88v0rwc.execute-api.us-west-2.amazonaws.com/
user:     gerald
password: gYDEij%L-VX5PApLRSr!


------------------------------------------------------------
------------------------------------------------------------
------------------------------------------------------------



please create a usecase agent to built out the following use-cases for iter (using voice-to-text, so some text cleanup may be required):

this is an architectural change: today each repo is expected to have its own ITER_engine which operates within the repo itself. This means if I have four different projects in four different repos all on one computer, I have to also run four different engines for those projects to be handled by the ITER_data web interface.  This is unnecessary overhead since one engine could run on one computer and manage the agents of multiple projects at the same time. There's no reason why we need physical isolation of the engine, running project a from the engine, running project B; we can just have logical isolation. This will require some change to the engine because now the engine will have the idea of multiple projects in addition to the iter_data server.



new project setup: I want the ability to create a new project on the UI/data application side that points to a particular code directory her server; that code Director needs to be different from one server to another because the code repo may be cloned in different places, depending on the user and the server. The on boarding of a new project should also include instructions on how to set up and configure the iter_engine, including a curl for download and a set of options that all turn into a final copy-able bash command.  Some of this exist today and just needs to be made more robust and more user-friendly. once the project is created, the user should be moved over to the project graph interface where they can start building the application nodes and edges that they need. For example, if a developer creates a new project for a dog walking app, they get the instructions on how to set up it on the engine, if needed


 
https://console.aws.amazon.com/billing/home#/bills?year=2026&month=9


Please build the next version of iter, using the iter/iter5/requirements.md as a guide.
You are allowed to look at past versions like iter4 for examples of how we built past versions, 
just keep in mind that there are many changes in this version. 

Ask any questions up-front, use agent teams to distribute the work for parallelism.

/goal rebuild the entire project, iter_engine, iter_data, as well as very thorough testing, since this is a big change with many potentially sync issues going from graph to file repo.  make sure it all works, API, MCP, and use playwright to make sure it all looks beautiful.  Do not stop until all tests pass green.



Two things are waiting on you, and neither blocks anything:
- pdy-dev's requirement split. One requirement per file turns its 136 requirement files into 6,628. Is that the granularity you want? If not, a coarser rule (for example, one file per heading section) is easy to switch to. The converted copy is in this session's scratchpad (6.7 GB), so you can browse it.
- Converting iter5's own node files. iter5's own node files are still in iter4 format. I haven't converted them in place; that's your call whenever you're ready.


let's do this:
- i do want to proceed with turning each atomic requirement into it's own file
- before we do however:
  - explode out "Several rules bundled in one entry" into individual entries
  - remove all "Rationale or history only" both from any stand-alone rules, but also stripped from existing rules; make them thin and effective, we can track history elsewhere
  - move all "Status / gap / "not built yet" notes into queued workitems, for planning and implementation.  remove them from requirements
  - rationalize all requirements, to make sure business is entered as a bizreq, and technical requirements are in techreqs.
    - move the 20 "Business rule / policy" from techreqs and into bizreqs
    - move the 142 "Technical constraint / architecture" from bizreqs and into techreqs
  - we got rid of interfaces, so make sure "Integration / interface" appear somewhere appropriate
  - rationalize all requirements again, to make sure global requirements appear in the global project_root/requirements/ folder, while requirements that only affect one context/container/component/connection are folded into the <object>/reqs/ folder
  - perform another full logical deduplication; make sure one requirement appears once
- i want all files to be placed in a subfolder called reqs/ or requirements/ which can hold both bizreqs and techreqs; this will prevent polluting the main repo filepath with too many *req.iter.md files
- I'm not worried about low component counts, as long as you confirm the below:
- please confirm: similar to iter4, iter5 should provide as agent context:
  - all reqs/ files (bizreq + techreq)
  - all reqs/ files for all ancestor paths 
  - any reqs referenced in the code.iter.md file
  - any reqs/ for decendants that are affected by any change
  - for example: 
    ```
    ↳ data\  
      ↳ code.iter.md (context)
      ↳ reqs\
        ↳ rule001.techreq.iter.md
        ↳ rule002.bizreq.iter.md
        ↳ rule003.techreq.iter.md
        ↳ rule004.bizreq.iter.md 
      ↳ postgres18\  
        ↳ code.iter.md (container)
        ↳ docker_setup_files\
        ↳ reqs\
          ↳ rule001.techreq.iter.md
          ↳ rule002.bizreq.iter.md
        ↳ encryption_extension\  
            ↳ code.iter.md (component)
            ↳ cpp_files\
            ↳ reqs\
              ↳ rule001.techreq.iter.md
              ↳ rule002.bizreq.iter.md
    ```
      - starting work on postgres18/ (container) would pull req files:
        - yes: all global
        - yes: posgtres18/reqs/* (self)
        - yes: data/reqs/* (ancestor)
        - maybe: encryption_extension/reqs/* (affected descendant, if agent feels needed)