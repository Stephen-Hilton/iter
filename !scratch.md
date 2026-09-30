cd ~/dev/pdy-dev && ~/dev/iterapp/iter3/bin/iter_engine --config .iter/config.json

https://ivi88v0rwc.execute-api.us-west-2.amazonaws.com/
user:     gerald
password: gYDEij%L-VX5PApLRSr!


------------------------------------------------------------
------------------------------------------------------------
------------------------------------------------------------



Decisions for you
1. Storing uploaded originals will push to GitHub. 
> not sure what you're asking me here.  The "iter_engine document storage directory" lives where-ever the setting tells it to, by default, `{topdir}/docs/` but that can be changed.  The user is also free to add any logic to their .gitignore file they want.  
> we could have another setting   `add "iter_engine document storage directory" to .gitignore` which does exactly that, as a convenience / reminder
2. The iter4-mac engine is still running on this laptop. It serves iter4 (still Stopped) with your Claude login.
> this is fine, I'll add some work, start the engine and test it shortly

Recommended next steps
- Give every agent session the MCP server automatically. The engine can pass --mcp-config with its token and the calling item's id, so agents search before reading files. Today they'd need an .mcp.json.
> agreed; re-write the agent prompt text (per agent or _shared) to point agents towards the MCP as needed, rather than embedding instructions in the prompts themselves.  Make sure you test to ensure the agents are using the MCPs after the change.
- Re-index after every map sync, not only on a schedule. Unchanged files cost nothing, so this is cheap.
> agreed; but arent map syncs happening every heartbeat, like every 5sec? 
- Link uploaded documents to map nodes they mention. Right now only node files come back with map context.
> agreed; we can create a new "children" list for "documents" (aside from those assigned, like bizreq or techreq)
- Add keyword search (ArangoSearch) next to the vectors, for exact names and error codes. The model also reads only the first 256 tokens of a chunk, so a long chunk's raw vector misses its tail.
> what does this mean?  "The model also reads only the first 256 tokens of a chunk"?  do you mean the 1000 characters risks dropping tokens? 
> I like the idea of ArangoSearch, aka use the text search capabilities to find chunks quickly.  Remember, each chunk should have an AI summary too
- Speed up bulk embedding in the container. It is 4–7× slower than a native build, even after a faster memory allocator. Search speed is fine; uploads of large documents are slow.
> we could move this to the engine, which runs natively on more powerful hardware (since it's doing the heavy-lifting of AI dev). 
> that would take some rebuilding, but makes sense for the iter_engine to own embedding (it already owns LLM calls).  thoughts?
- Missing formats: OCR for scanned PDFs, plus pptx/xlsx.
> are you saying PDF, PPTx, XLSX are missing?   I don't care about XLSX, but PPTx and PDF should be covered
- Guard the cost of summaries. They are recorded as spend (agent: summary) but not held to the project's daily limit.
> na, that's fine; it'll be a rare occurance most of the time
- Measure search quality with a small set of real questions to track whether changes help.
> are you advocating asking the users?  or Agents?




  1. Add it on claude.ai. Go to claude.ai → Settings → Connectors, find Google Slides and connect it with your Google account. If it isn't listed there,
     Google Slides isn't offered as a connector for your account, and we should switch to option 2 (a .pptx you import).
  2. Load it in Claude Code. Type /mcp here and check that a "claude.ai Google Slides" server appears and is enabled. If it asks you to authenticate, finish
     that step. If it doesn't appear at all, restart Claude Code (for example with claude --continue) so it reloads your connectors.
  3. Tell me when it's on. I'll check that the Slides tools are loaded, read your resume, the job description and the iter4 material, then build the deck
     directly in your existing file with the same link.

# ------------
# ------------ slideware prompt
# ------------


I'm gearing up for a technical interview with ArangoDB as a Solution Engineer, where I'm expected to conduct a faux-technical sales call.
I would like help authoring a beautiful deck to help me drive the meeting. 

Here is my resume:  https://stephen.skillsnap.me/resume.pdf 
Here is the job description:  https://arangodb.applytojob.com/apply/PEZtXlyo96/Senior-Solutions-Engineer-Presales
Here are the instructions on my presentation:
```
Please demonstrate a product you are selling, a technical RAG project (VectorRAG/GraphRAG) or Arango in the form of a "live" sales meeting (bonus points for ArangoDB with GraphRAG). Be prepared to review how you built the project/how the product is built, the decisions made, and the options to tune performance and results. The ADB Team will role-play (e.g., buyer, engineer, procurement) with live questions during the session.
```

I want to use Iter4 work as my topic, as well as showing an early version in production with https://skillsnap.me

Another agent has already started a top-notch, very beautiful presentation with: 
- title slide
- my background, in 1 slide, highlighting why i'm the right candidate 

But it didn't have access to iter4, so it went a little off-the-rails after that.  please modify the slides with:
- my 20-minute presentation trying to "sell" iter4, and explaining how it works
- slide to invite Q&A
- Thank you slide at the end

here is the google slides document, editable:  https://docs.google.com/presentation/d/1G-zHkKq_-deJSzXla5AD3eEc4BHT44uCnSsQ1Clw8Lo/edit?usp=sharing

Please generate a first draft, but make it look good!