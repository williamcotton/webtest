<?php
if (getenv('WEBTEST') !== '1') throw new Exception('test-only bridge');
$manifest = json_decode(file_get_contents(__DIR__.'/.webtest/app-schema.json'), true);
$users = [];
$port = getenv('PORT') ?: '3109';
$server = stream_socket_server("tcp://127.0.0.1:$port", $errno, $error);
$endpoint = parse_url(getenv('WEBTEST_BRIDGE'));
if (($endpoint['scheme'] ?? '') !== 'tcp' || $endpoint['host'] !== '127.0.0.1') throw new Exception('loopback TCP required');
$bridge = stream_socket_client("tcp://{$endpoint['host']}:{$endpoint['port']}", $errno, $error, 10);
function send_frame($stream, $value) { fwrite($stream, json_encode($value, JSON_UNESCAPED_UNICODE)."\n"); fflush($stream); }
send_frame($bridge, ['type'=>'hello','protocol_versions'=>[1],'sdk'=>'webtest-php-example','sdk_version'=>'0.1.0','token'=>getenv('WEBTEST_TOKEN'),'capabilities'=>['cancel'=>false,'events'=>false]]);
$hello = json_decode(fgets($bridge), true); if (($hello['type'] ?? '') !== 'hello_ok') exit(2);
while (true) {
  $read=[$server,$bridge];$write=$except=[];if(stream_select($read,$write,$except,null)===false)break;
  foreach($read as $ready){
    if($ready===$server){$client=stream_socket_accept($server);$request='';while(!str_contains($request,"\r\n\r\n")){$request.=fread($client,8192);}
      [$headers,$body]=array_pad(explode("\r\n\r\n",$request,2),2,'');$length=preg_match('/^content-length:\s*(\d+)\r?$/im',$headers,$match)?(int)$match[1]:0;
      while(strlen($body)<$length){$body.=fread($client,$length-strlen($body));}$line=strtok($headers,"\r\n");
      if(str_starts_with($line,'GET /health '))$content='ok';
      elseif(str_starts_with($line,'GET /login '))$content='<form method="post"><label>Email <input name="email"></label><button>Sign in</button></form>';
      elseif(str_starts_with($line,'POST /login ')){parse_str($body,$form);$email=$form['email']??'';$content=isset($users[$email])?"<p>Welcome, $email</p>":'<p>Invalid sign in</p>';}else{$content='not found';}
      fwrite($client,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: ".strlen($content)."\r\nConnection: close\r\n\r\n$content");fclose($client);
    } else {$line=fgets($bridge);if($line===false)break 2;$message=json_decode($line,true);$kind=$message['type']??'';$id=$message['id']??0;
      if($kind==='describe')send_frame($bridge,['type'=>'schema','id'=>$id,'protocol'=>1,'schema_hash'=>$manifest['schema_hash'],'functions'=>$manifest['functions']]);
      elseif($kind==='call'){$args=$message['arguments'];$email=$args['email'];if(isset($users[$email]))send_frame($bridge,['type'=>'error','id'=>$id,'code'=>'user.email_taken','message'=>'email already exists','retryable'=>false,'data'=>(object)[]]);else{$user=['id'=>count($users)+1,'email'=>$email,'admin'=>$args['admin']??false];$users[$email]=$user;send_frame($bridge,['type'=>'result','id'=>$id,'value'=>$user]);}}
      elseif($kind==='ping')send_frame($bridge,['type'=>'pong','id'=>$id]);
      elseif($kind==='shutdown'){send_frame($bridge,['type'=>'shutdown_ok','id'=>$id]);break 2;}
    }
  }
}
fclose($bridge);fclose($server);
?>
