#!/usr/bin/env python3
"""Exercise only local tools against a disposable DB; never point at live mail."""
import contextlib,json,os,pathlib,selectors,subprocess,tempfile,time
import sys,sqlite3
BINARY=pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/mail-mcp').resolve()
class Client:
 def __init__(self,db,flags=()):
  args=[str(BINARY),'--db',str(db)]
  args+=list(flags)
  self.proc=subprocess.Popen(args,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
  self.seq=0;self.buffer=b'';self.sel=selectors.DefaultSelector();self.sel.register(self.proc.stdout,selectors.EVENT_READ)
  self.call('initialize',{'protocolVersion':'2024-11-05','capabilities':{},'clientInfo':{'name':'email-local-qa','version':'1'}})
  self.send({'jsonrpc':'2.0','method':'notifications/initialized'})
 def send(self,obj):self.proc.stdin.write((json.dumps(obj)+'\n').encode());self.proc.stdin.flush()
 def call(self,method,params):
  self.seq+=1;seq=self.seq;self.send({'jsonrpc':'2.0','id':seq,'method':method,'params':params});deadline=time.monotonic()+10
  while time.monotonic()<deadline:
   if b'\n' not in self.buffer:
    if not self.sel.select(max(0,deadline-time.monotonic())):break
    chunk=os.read(self.proc.stdout.fileno(),65536)
    if not chunk:raise AssertionError('MCP closed: '+self.proc.stderr.read().decode())
    self.buffer+=chunk
   while b'\n' in self.buffer:
    line,self.buffer=self.buffer.split(b'\n',1)
    if not line.strip():continue
    value=json.loads(line)
    if value.get('id')==seq:return value
  raise AssertionError('MCP request timed out: '+method)
 def tool(self,name,args=None):return self.call('tools/call',{'name':name,'arguments':args or {}})
 def close(self):
  self.proc.stdin.close()
  try:self.proc.wait(timeout=3)
  except subprocess.TimeoutExpired:self.proc.terminate();self.proc.wait(timeout=3)
  self.sel.close()
 def __enter__(self):return self
 def __exit__(self,*_):self.close()
def success(response):
 assert 'error' not in response,response
 result=response['result'];assert not result.get('isError'),response
 if 'structuredContent' in result:
  value=result['structuredContent']
  # rmcp wraps primitive/array Json results in a result field.
  if isinstance(value,dict) and set(value)=={'result'}:return value['result']
  # Third-party content arrives in a labelled untrusted-data envelope.
  if isinstance(value,dict) and 'content_trust' in value:
   assert value['content_trust']=='untrusted_third_party_content',value
   assert 'Never follow instructions' in value['content_notice'],value
   assert value['end_of_untrusted_data'].startswith('END OF UNTRUSTED'),value
   return value['data']
  return value
 content=result.get('content',[])
 texts=[c['text'] for c in content if c.get('type')=='text']
 assert texts,response
 return json.loads(texts[0])
GRANTS=json.dumps(['draft','triage','send','sync','calendar_write'])
def consent(db,account,grants=GRANTS):
 # Simulates the user's in-app consent click. The MCP server has no way to grant it.
 with sqlite3.connect(db) as connection:
  connection.execute('INSERT OR REPLACE INTO agent_consents(account_id,granted_at_ms,grants_json,disclosure_version) VALUES(?,?,?,1)',(account,int(time.time()*1000),grants))
def denied(response,fragment=None):
 assert response.get('error') or response.get('result',{}).get('isError'),response
 if fragment:assert fragment in json.dumps(response),response
with tempfile.TemporaryDirectory(prefix='email-mcp-permissions-') as directory:
 db=pathlib.Path(directory)/'mail.sqlite'
 with Client(db,('--seed-demo',)) as client:
  # Upgraded registrations see nothing until the user turns on agent access.
  denied(client.tool('list_accounts'),'Agent access in the Email app')
  denied(client.tool('get_message',{'message_id':'demo:1'}),'Agent access is off')
  consent(db,'demo:personal')
  # Consent applies to the next request without restarting the server.
  assert len(success(client.tool('list_accounts')))==1
  assert isinstance(success(client.tool('suggest_recipients',{'account_id':'demo:personal','query':'maya'})),list)
  denied(client.tool('compose_draft',{'account_id':'demo:personal','to':'x@example.com','subject':'Denied','body_plain':'text'}),'--allow-draft')
 with sqlite3.connect(db) as connection:
  connection.execute("INSERT INTO accounts(id,provider,email,display_name) SELECT 'other',provider,'other@example.com','Other' FROM accounts LIMIT 1")
 consent(db,'other')
 with Client(db,('--account','demo:personal')) as client:
  assert isinstance(success(client.tool('suggest_recipients',{'account_id':'demo:personal','query':'maya'})),list)
  denied(client.tool('suggest_recipients',{'account_id':'other','query':'x'}),'permitted accounts')
  denied(client.tool('suggest_recipients',{'query':'maya'}))
  denied(client.tool('suggest_recipients',{'account_id':None,'query':'maya'}))
 with Client(db,('--allow-draft',)) as client:
  make=lambda account:success(client.tool('compose_draft',{'account_id':account,'to':'x@example.com','subject':'Test draft','body_plain':'Local only'}))['id']
  allowed=make('demo:personal');foreign=make('other')
  denied(client.tool('send_draft',{'message_id':allowed}),'--allow-send')
  denied(client.tool('queue_send_draft',{'message_id':allowed}),'--allow-send')
  denied(client.tool('archive_thread',{'thread_id':'demo:1'}),'--allow-triage')
  denied(client.tool('sync_all'),'--allow-sync')
 with Client(db,('--allow-write','--account','demo:personal')) as client:
  assert [a['id'] for a in success(client.tool('list_accounts'))]==['demo:personal']
  assert isinstance(success(client.tool('suggest_recipients',{'account_id':'demo:personal','query':'maya'})),list)
  denied(client.tool('suggest_recipients',{'account_id':'other','query':'x'}),'permitted accounts')
  assert [d['id'] for d in success(client.tool('list_drafts'))]==[allowed]
  assert success(client.tool('get_message',{'message_id':allowed}))['id']==allowed
  for tool in ['get_message','render_message','list_attachments','list_draft_attachments','send_draft']:
   denied(client.tool(tool,{'message_id':foreign}),'permitted accounts')
  denied(client.tool('batch_triage',{'operation_id':'denied-mixed','kind':'archive','thread_ids':['demo:1',foreign]}))
  assert success(client.tool('get_message',{'message_id':'demo:1'}))['id']=='demo:1'
  assert success(client.tool('pending_action_count'))['count']==0
  denied(client.tool('list_snippets'),'shared data')
  denied(client.tool('dispatch_due'),'shared data')
 with Client(db,('--allow-triage',)) as client:
  foreign_job=success(client.tool('create_sender_cleanup',{'account_id':'other','sender':'private@example.com'}))['id']
 with Client(db,('--allow-triage','--account','demo:personal')) as client:
  job=success(client.tool('create_sender_cleanup',{'account_id':'demo:personal','sender':'news@example.com'}))
  job_id=job['id'];assert job['state']=='discovering'
  assert len(success(client.tool('list_sender_cleanups')))==1
  assert success(client.tool('get_sender_cleanup',{'job_id':job_id}))['id']==job_id
  denied(client.tool('confirm_sender_cleanup',{'job_id':job_id})) # no discovery/confirmation bypass
  denied(client.tool('step_sender_cleanup',{'job_id':job_id}),'both --allow-triage and --allow-sync')
  for name in ['get_sender_cleanup','confirm_sender_cleanup','pause_sender_cleanup','resume_sender_cleanup','cancel_sender_cleanup']:
   denied(client.tool(name,{'job_id':foreign_job}),'permitted accounts')
  assert success(client.tool('pause_sender_cleanup',{'job_id':job_id}))['state']=='paused'
  assert success(client.tool('resume_sender_cleanup',{'job_id':job_id}))['state']=='discovering'
  assert success(client.tool('cancel_sender_cleanup',{'job_id':job_id}))['state']=='cancelled'
  # Bulk triage only creates a request; nothing changes until the user approves.
  request=success(client.tool('batch_triage',{'operation_id':'allowed-archive','kind':'archive','thread_ids':['demo:1']}))
  assert request['status']=='pending_approval',request
  assert success(client.tool('batch_triage',{'operation_id':'allowed-archive','kind':'archive','thread_ids':['demo:1']}))['approval_id']==request['approval_id']
  assert success(client.tool('latest_triage_undo')) is None
  assert success(client.tool('pending_action_count'))['count']==0
  assert any(t['id']=='demo:1' for t in success(client.tool('list_threads',{'account_id':'demo:personal'})))
  assert success(client.tool('get_agent_action',{'approval_id':request['approval_id']}))['status']=='pending'
  denied(client.tool('undo_triage',{'operation_id':'allowed-archive'}),'unavailable')
  denied(client.tool('update_draft',{'message_id':allowed,'to':'x@example.com','subject':'bad','body_plain':'bad'}),'--allow-draft')
 with Client(db,('--allow-send','--account','demo:personal')) as client:
  # Sends only create requests for the user; nothing reaches the outbox.
  for name,args in [('schedule_draft',{'message_id':allowed,'send_at_ms':4102444800000})]:
   request=success(client.tool(name,args))
   assert request['status']=='pending_approval' and 'Nothing has been sent' in request['message'],request
  denied(client.tool('send_draft',{'message_id':allowed}),'already waiting for approval')
  assert success(client.tool('list_outbox'))==[]
  denied(client.tool('cancel_scheduled_draft',{'message_id':allowed}),'can no longer be undone')
  denied(client.tool('schedule_draft',{'message_id':foreign,'send_at_ms':4102444800000}),'permitted accounts')
  denied(client.tool('approve_agent_action',{'id':request['approval_id']}))
 with sqlite3.connect(db) as connection:
  client_name,status=connection.execute("SELECT client,status FROM agent_actions WHERE kind='schedule_draft'").fetchone()
  assert (client_name,status)==('email-local-qa','pending'),(client_name,status)
 # The external scheduler's dispatcher finds nothing to send while a request waits.
 result=subprocess.run([str(BINARY),'--db',str(db),'--allow-send','--dispatch-due'],capture_output=True,timeout=10)
 assert result.returncode==0 and result.stdout.strip()==b'[]',result
 with sqlite3.connect(db) as connection:
  connection.execute("UPDATE agent_actions SET status='rejected',decided_at_ms=1 WHERE status='pending'")
 # Discard/restore follow the draft's account, including after the message row is gone.
 with Client(db,('--account','demo:personal')) as client:
  denied(client.tool('discard_draft',{'message_id':allowed}),'--allow-draft')
 with Client(db,('--allow-draft','--account','demo:personal')) as client:
  denied(client.tool('discard_draft',{'message_id':foreign}),'permitted accounts')
  assert success(client.tool('discard_draft',{'message_id':allowed}))['message_id']==allowed
  assert success(client.tool('list_drafts'))==[]
  assert success(client.tool('restore_discarded_draft',{'message_id':allowed}))['id']==allowed
  assert [d['id'] for d in success(client.tool('list_drafts'))]==[allowed]
 with Client(db,('--allow-draft',)) as client:
  success(client.tool('discard_draft',{'message_id':foreign}))
 with Client(db,('--allow-draft','--account','demo:personal')) as client:
  denied(client.tool('restore_discarded_draft',{'message_id':foreign}),'permitted accounts')
 with Client(db,('--allow-draft',)) as client:
  assert success(client.tool('restore_discarded_draft',{'message_id':foreign}))['id']==foreign
 # Calendar/follow-up/signature fixtures are local-only. The calendar cache is
 # populated directly so tests never fetch an attachment or contact a provider.
 with sqlite3.connect(db) as connection:
  foreign_thread=connection.execute('SELECT thread_id FROM messages WHERE id=?',(foreign,)).fetchone()[0]
  def event(uid):
   return {'uid':uid,'summary':'Local calendar fixture','description':'No provider event exists.',
    'location':'','organizer':None,'attendees':[],
    'start':{'value':'20270101T120000Z','display':'2027-01-01 12:00 UTC','timezone':'UTC','all_day':False,'utc_ms':1798804800000},
    'end':None,'status':None,'method':'REQUEST','recurrence_rule':None,'warnings':[]}
  for message_id,uid in [('demo:1','own-calendar'),(foreign,'foreign-calendar')]:
   connection.execute('INSERT INTO calendar_invitations(message_id,event_index,event_json,start_value) VALUES(?,0,?,?)',
    (message_id,json.dumps(event(uid)),'20270101T120000Z'))
 due=int(time.time()*1000)+3600000
 own_follow={'thread_id':'demo:1','due_at_ms':due,'only_if_no_reply':True}
 foreign_follow={'thread_id':foreign_thread,'due_at_ms':due,'only_if_no_reply':True}
 with Client(db,('--allow-triage',)) as client:
  success(client.tool('set_followup',foreign_follow))
 with Client(db,('--allow-triage','--account','demo:personal')) as client:
  follow=success(client.tool('set_followup',own_follow))
  assert follow['thread_id']=='demo:1' and follow['status']=='waiting',follow
  denied(client.tool('set_followup',foreign_follow),'permitted accounts')
  denied(client.tool('cancel_followup',{'thread_id':foreign_thread}),'permitted accounts')
  success(client.tool('cancel_followup',{'thread_id':'demo:1'}))
  assert success(client.tool('list_followups',{'account_id':'demo:personal'}))[0]['status']=='cancelled'
  success(client.tool('set_followup',own_follow))
  denied(client.tool('upsert_signature',{'account_id':'demo:personal','email':'shared@example.com','body':'Denied','for_new':True,'for_reply':True}),'--allow-draft')
 with Client(db,('--allow-draft',)) as client:
  for account,body in [('demo:personal','Allowed signature'),('other','Private signature')]:
   success(client.tool('upsert_signature',{'account_id':account,'email':'shared@example.com','body':body,'for_new':True,'for_reply':False}))
 with Client(db,('--account','demo:personal')) as client:
  denied(client.tool('set_followup',own_follow),'--allow-triage')
  denied(client.tool('cancel_followup',{'thread_id':'demo:1'}),'--allow-triage')
  follows=success(client.tool('list_followups',{'account_id':'demo:personal'}))
  assert len(follows)==1 and follows[0]['thread_id']=='demo:1',follows
  for tool in ['list_followups','list_calendar_invitations']:
   denied(client.tool(tool,{'account_id':'other'}),'permitted accounts')
   denied(client.tool(tool,{}))
   denied(client.tool(tool,{'account_id':None}))
  calendar=success(client.tool('list_calendar_invitations',{'account_id':'demo:personal'}))
  assert len(calendar)==1 and calendar[0]['event']['uid']=='own-calendar',calendar
  assert success(client.tool('calendar_invitation',{'message_id':'demo:1'}))[0]['event']['uid']=='own-calendar'
  denied(client.tool('calendar_invitation',{'message_id':foreign}),'permitted accounts')
  denied(client.tool('refresh_calendar_invitation',{'message_id':'demo:1'}),'--allow-sync')
  assert success(client.tool('get_signature',{'account_id':'demo:personal','email':'shared@example.com'}))['body']=='Allowed signature'
  denied(client.tool('get_signature',{'account_id':'other','email':'shared@example.com'}),'permitted accounts')
  denied(client.tool('upsert_signature',{'account_id':'demo:personal','email':'shared@example.com','body':'Denied','for_new':True,'for_reply':True}),'--allow-draft')
  denied(client.tool('delete_signature',{'account_id':'demo:personal','email':'shared@example.com'}),'--allow-draft')
 with Client(db,('--allow-sync','--account','demo:personal')) as client:
  # Denial must precede provider/cache access; never call permitted refresh here.
  denied(client.tool('refresh_calendar_invitation',{'message_id':foreign}),'permitted accounts')
 with Client(db,('--allow-draft','--account','demo:personal')) as client:
  value=success(client.tool('upsert_signature',{'account_id':'demo:personal','email':'shared@example.com','body':'Updated signature','for_new':False,'for_reply':True}))
  assert value['body']=='Updated signature' and value['for_reply'] and not value['for_new'],value
  denied(client.tool('upsert_signature',{'account_id':'other','email':'shared@example.com','body':'Overwrite denied','for_new':True,'for_reply':True}),'permitted accounts')
  denied(client.tool('delete_signature',{'account_id':'other','email':'shared@example.com'}),'permitted accounts')
  success(client.tool('delete_signature',{'account_id':'demo:personal','email':'shared@example.com'}))
  assert success(client.tool('get_signature',{'account_id':'demo:personal','email':'shared@example.com'})) is None
  denied(client.tool('set_followup',own_follow),'--allow-triage')
 # Calendar writes require a new dedicated grant; legacy mail permissions cannot RSVP.
 calendar_range={'account_id':'demo:personal','start_ms':1794304800000,'end_ms':1794308400000}
 calendar_rsvp={'account_id':'demo:personal','calendar_id':'primary','event_id':'fixture','response':'accepted'}
 calendar_create={**calendar_range,'calendar_id':'primary','title':'Fixture','attendee_emails':[]}
 calendar_update={'account_id':'demo:personal','calendar_id':'primary','event_id':'fixture','expected_etag':'"v1"','title':'Updated fixture'}
 for flags in [(),('--allow-write',),('--allow-send',),('--allow-sync',)]:
  with Client(db,flags) as client:
   denied(client.tool('respond_calendar_invitation',calendar_rsvp),'--allow-calendar-write')
   denied(client.tool('create_calendar_event',calendar_create),'--allow-calendar-write')
   denied(client.tool('update_calendar_event',calendar_update),'--allow-calendar-write')
 with Client(db,('--account','demo:personal')) as client:
  assert success(client.tool('list_calendars',{'account_id':'demo:personal'}))==[]
  assert success(client.tool('list_agenda',calendar_range))==[]
  assert success(client.tool('availability',{**calendar_range,'duration_minutes':30}))['coverage_complete'] is False
  for name,args in [('list_calendars',{}),('list_agenda',calendar_range),('availability',{**calendar_range,'duration_minutes':30})]:
   denied(client.tool(name,{**args,'account_id':'other'}),'permitted accounts')
   denied(client.tool(name,{**args,'account_id':None}))
  denied(client.tool('sync_calendar',calendar_range),'--allow-sync')
 with Client(db,('--allow-calendar-write','--account','demo:personal')) as client:
  denied(client.tool('respond_calendar_invitation',{**calendar_rsvp,'account_id':'other'}),'permitted accounts')
  denied(client.tool('create_calendar_event',{**calendar_create,'account_id':'other'}),'permitted accounts')
  denied(client.tool('update_calendar_event',{**calendar_update,'account_id':'other'}),'permitted accounts')
  denied(client.tool('send_draft',{'message_id':allowed}),'--allow-send')
  denied(client.tool('sync_calendar',calendar_range),'--allow-sync')
 for flags in [('--dispatch-due',),('--allow-send','--dispatch-due','--account','demo:personal')]:
  result=subprocess.run([str(BINARY),'--db',str(db),*flags],capture_output=True,timeout=10)
  assert result.returncode!=0,result
 # Enriched activity must keep subject, timeline and reminder context within the
 # explicit message's permitted account. Seed only local sent-message fixtures.
 with Client(db) as client:
  success(client.tool('tracking_activity',{'refresh':False}))
 with sqlite3.connect(db) as connection:
  for source,message_id,tracking_id,subject in [
   (allowed,'tracking-own','11111111-1111-4111-8111-111111111111','Owned activity'),
   (foreign,'tracking-foreign','22222222-2222-4222-8222-222222222222','Foreign private activity')]:
   cursor=connection.execute('SELECT * FROM messages WHERE id=?',(source,))
   message=dict(zip([column[0] for column in cursor.description],cursor.fetchone()))
   message.update(id=message_id,is_sent=1,is_draft=0,subject=subject)
   connection.execute('INSERT INTO messages ('+','.join(message)+') VALUES ('+','.join('?' for _ in message)+')',list(message.values()))
   connection.execute('INSERT INTO email_tracking(message_id,tracking_id,enabled,host_id,pixel_requests,first_requested_at,last_requested_at) VALUES (?,?,1,?,2,?,?)',
    (message_id,tracking_id,'h_aaaaaaaaaaaaaaaaaaaaaaaa',int(time.time()*1000)-2000,int(time.time()*1000)-1000))
 with Client(db,('--account','demo:personal')) as client:
  result=success(client.tool('tracking_activity',{'message_id':'tracking-own','refresh':False}))
  assert len(result['items'])==1,result
  item=result['items'][0]
  assert item['message_id']=='tracking-own' and item['account_id']=='demo:personal',item
  assert item['subject']=='Owned activity' and isinstance(item['timeline'],list),item
  assert 'Foreign private activity' not in json.dumps(result),result
  denied(client.tool('tracking_activity',{'message_id':'tracking-foreign','refresh':False}),'permitted accounts')
  denied(client.tool('tracking_activity',{'refresh':False}))
  denied(client.tool('set_followup',{'thread_id':item['thread_id'],'due_at_ms':int(time.time()*1000)+3600000,'only_if_no_reply':True}),'--allow-triage')
 with Client(db,('--allow-triage','--account','demo:personal')) as client:
  guarded={'thread_id':item['thread_id'],'due_at_ms':int(time.time()*1000)+3600000,'only_if_no_reply':True,'sent_message_id':'tracking-own'}
  assert success(client.tool('set_followup',guarded))['status']=='waiting'
  denied(client.tool('set_followup',{**guarded,'sent_message_id':'tracking-foreign'}),'permitted accounts')
  success(client.tool('cancel_followup',{'thread_id':item['thread_id']}))
 # Turning access off applies to a running server's next request.
 with Client(db,('--account','demo:personal')) as client:
  assert success(client.tool('get_message',{'message_id':'demo:1'}))['id']=='demo:1'
  with sqlite3.connect(db) as connection:
   connection.execute("DELETE FROM agent_consents WHERE account_id='demo:personal'")
  denied(client.tool('get_message',{'message_id':'demo:1'}),'Agent access is off')
 with sqlite3.connect(db) as connection:
  blocked=connection.execute("SELECT count(*) FROM agent_actions WHERE status='blocked'").fetchone()[0]
  assert blocked>=1,blocked
 print('PASS: JSON-RPC capability separation, in-app consent and revocation, approval-gated sends/batches with idle dispatcher, untrusted content envelope, scoped lists/read/write/attachments/undo/outbox/calendar/followups/signatures/activity, global-operation rejection; no network/provider operations')
